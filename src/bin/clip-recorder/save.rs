//! Ctrl+Alt+R: the last N seconds as an MP4.
//!
//! The quick part runs on the UI thread (pick segments, snapshot them, write the audio WAVs),
//! so the window is fixed at the press; ffmpeg's concat, mix and mux run on a worker thread.
//! Every way this can end is reported: a lost clip once looked exactly like a saved one.

use crate::audio::{Audio, Kind};
use crate::capture::{CREATE_NO_WINDOW, Capture};
use crate::logging::log;
use clip_recorder::clock;
use clip_recorder::ffmpeg_cmd::{self, SEGMENT_LIST_FILE};
use clip_recorder::selection::{self, GiveUp, Segment};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30);

/// Seconds of footage saved, or why not.
pub type Outcome = Result<u32, GiveUp>;

/// Starts a save. `done` runs on the UI thread once, whatever happens.
pub fn start(capture: &Capture, audio: &Audio, ffmpeg: &Path, clip_seconds: u32, folder: &Path, done: fn(Outcome)) {
    let save_time = clock::now();
    let id = format!("{:08x}", (save_time.fract() * 1e9) as u64 ^ u64::from(std::process::id()));
    match prepare(capture, audio, ffmpeg, clip_seconds, folder, save_time, &id) {
        Ok(job) => {
            std::thread::spawn(move || {
                let outcome = job.run();
                let _ = slint::invoke_from_event_loop(move || done(outcome));
            });
        }
        // Deferred even here: the caller is still borrowing the app state `done` will need.
        Err(reason) => {
            let _ = slint::invoke_from_event_loop(move || done(Err(reason)));
        }
    }
}

/// Everything the worker needs, decided at the press.
struct Job {
    id: String,
    ffmpeg: PathBuf,
    duration: f64,
    list: PathBuf,
    snapshots: Vec<PathBuf>,
    loopback: Option<PathBuf>,
    mic: Option<PathBuf>,
    work: PathBuf,
    output: PathBuf,
    started: Instant,
}

fn prepare(
    capture: &Capture,
    audio: &Audio,
    ffmpeg: &Path,
    clip_seconds: u32,
    folder: &Path,
    save_time: f64,
    id: &str,
) -> Result<Job, GiveUp> {
    let started = Instant::now();
    let dir = capture.segment_dir();
    let say = |msg: String| log(&format!("[{id}] {msg}"));
    let entries = std::fs::read_dir(dir).map_err(|e| {
        say(format!("ABORT: cannot list {}: {e}", dir.display()));
        GiveUp::Error
    })?;
    let segments: Vec<Segment> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let mtime = e.metadata().ok()?.modified().ok()?;
            (name.starts_with("seg_") && name.ends_with(".ts")).then(|| Segment { name, mtime: clock::seconds(mtime) })
        })
        .collect();
    let listed = match capture.clock_zero() {
        Some(zero) => selection::parse_segment_list(&std::fs::read_to_string(dir.join(SEGMENT_LIST_FILE)).unwrap_or_default(), zero),
        None => Default::default(),
    };
    let chosen = selection::select(&segments, &listed, save_time, clip_seconds, save_time - capture.started_at()).inspect_err(|r| {
        say(format!("ABORT: {r:?} ({} segments, capture started {:.1}s ago)", segments.len(), save_time - capture.started_at()))
    })?;
    say(format!(
        "selected {} segments, duration={:.3}s (requested {clip_seconds}s), anchor={} mtime_lag={:+.0}ms",
        chosen.segments.len(),
        chosen.duration,
        if chosen.captured { "capture" } else { "mtime" },
        chosen.mtime_lag * 1000.0
    ));

    // Snapshots: the live ffmpeg overwrites old names under -segment_wrap, and the concat reads
    // them seconds later. It can never touch a name it never wrote.
    let mut snapshots = Vec::new();
    for (i, s) in chosen.segments.iter().enumerate() {
        let snap = dir.join(format!("snap_{id}_{i:03}.ts"));
        if let Err(e) = std::fs::copy(dir.join(&s.name), &snap) {
            say(format!("ABORT: snapshot of {} failed: {e}", s.name));
            snapshots.iter().for_each(|p| drop(std::fs::remove_file(p)));
            return Err(GiveUp::Error);
        }
        snapshots.push(snap);
    }
    let list = dir.join(format!("concat_{id}.txt"));
    if std::fs::write(&list, ffmpeg_cmd::concat_list(&snapshots)).is_err() || std::fs::create_dir_all(folder).is_err() {
        say("ABORT: concat list or output folder not writable".into());
        snapshots.iter().for_each(|p| drop(std::fs::remove_file(p)));
        return Err(GiveUp::Error);
    }

    // The audio window is the video's: same start, same length.
    let wav = |kind: Kind| -> Option<PathBuf> {
        if !audio.has(kind) {
            return None;
        }
        say(audio.describe(kind));
        let path = dir.join(format!("{}_{id}.wav", if kind == Kind::Loopback { "loopback" } else { "mic" }));
        let ok = audio.save_wav(kind, &path, save_time, chosen.duration);
        say(format!("save_wav({kind:?}) ok={ok}"));
        ok.then_some(path)
    };
    let loopback = wav(Kind::Loopback);
    let mic = wav(Kind::Mic);
    Ok(Job {
        id: id.into(),
        ffmpeg: ffmpeg.into(),
        duration: chosen.duration,
        list,
        snapshots,
        loopback,
        mic,
        work: dir.into(),
        output: folder.join(format!("Clip_{}.mp4", local_stamp())),
        started,
    })
}

impl Job {
    fn say(&self, msg: &str) {
        log(&format!("[{}] {msg}", self.id));
    }

    fn run(self) -> Outcome {
        let has_audio = self.loopback.is_some() || self.mic.is_some();
        let video = if has_audio { self.work.join(format!("video_{}.mp4", self.id)) } else { self.output.clone() };
        // Two saves in the same second share the output name: only the run that wrote it may
        // judge or delete it.
        let mut wrote_output = false;
        let mut output_ok = false;
        let concat_ok = self.ffmpeg("concat", &ffmpeg_cmd::concat_args(&self.list, self.duration, &video));
        if !has_audio {
            wrote_output = true;
            output_ok = concat_ok;
        }
        let mut temp = vec![self.list.clone()];
        if has_audio && concat_ok {
            let audio = match (&self.loopback, &self.mic) {
                (Some(lb), Some(mic)) => {
                    let mixed = self.work.join(format!("mixed_{}.wav", self.id));
                    temp.push(mixed.clone());
                    // A failed or timed-out mix keeps the game sound rather than losing the clip
                    // (and never trusts a partial mixed WAV).
                    if self.ffmpeg("mix", &ffmpeg_cmd::mix_args(lb, mic, &mixed)) && mixed.is_file() { mixed } else { lb.clone() }
                }
                (Some(only), None) | (None, Some(only)) => only.clone(),
                (None, None) => unreachable!(),
            };
            wrote_output = true;
            output_ok = self.ffmpeg("mux", &ffmpeg_cmd::mux_args(&video, &audio, &self.output));
        }
        let saved = wrote_output && output_ok && std::fs::metadata(&self.output).is_ok_and(|m| m.len() > 0);
        temp.extend(self.snapshots.iter().cloned());
        temp.extend(self.loopback.iter().chain(self.mic.iter()).cloned());
        if has_audio {
            temp.push(video);
        }
        temp.iter().for_each(|p| drop(std::fs::remove_file(p)));
        if !saved && wrote_output {
            let _ = std::fs::remove_file(&self.output);
        }
        self.say(&format!("done: saved={saved} output={} took={:.1}s", self.output.display(), self.started.elapsed().as_secs_f64()));
        if saved { Ok(self.duration.round() as u32) } else { Err(GiveUp::Error) }
    }

    /// Runs one ffmpeg with a timeout; logs its exit and, on failure, its stderr's tail.
    fn ffmpeg(&self, what: &str, args: &[String]) -> bool {
        let child = Command::new(&self.ffmpeg)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            self.say(&format!("{what}: ffmpeg did not start"));
            return false;
        };
        let mut stderr = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_string(&mut text);
            }
            text
        });
        let deadline = Instant::now() + FFMPEG_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let text = reader.join().unwrap_or_default();
        let ok = status.is_some_and(|s| s.success());
        if ok {
            self.say(&format!("{what}: ok"));
        } else {
            let tail: String = text.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
            self.say(&format!("{what}: FAILED ({}): {tail}", status.map_or("timed out".into(), |s| s.to_string())));
        }
        ok
    }
}

fn local_stamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}
