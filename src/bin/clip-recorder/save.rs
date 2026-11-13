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
    let id = crate::winutil::random_hex();
    let source = Source {
        dir: capture.segment_dir(),
        clock_zero: capture.clock_zero(),
        started_at: capture.started_at(),
    };
    let dir = source.dir.to_path_buf();
    let mut wav = |kind: Kind, end: f64, duration: f64| -> Option<PathBuf> {
        if !audio.has(kind) {
            return None;
        }
        log(&format!("[{id}] {}", audio.describe(kind)));
        let path = dir.join(format!(
            "{}_{id}.wav",
            if kind == Kind::Loopback { "loopback" } else { "mic" }
        ));
        let ok = audio.save_wav(kind, &path, end, duration);
        log(&format!("[{id}] save_wav({kind:?}) ok={ok}"));
        ok.then_some(path)
    };
    match prepare(&source, &mut wav, ffmpeg, clip_seconds, folder, save_time, &id) {
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

/// What a save reads from the capture, as plain values.
struct Source<'a> {
    dir: &'a Path,
    clock_zero: Option<f64>,
    started_at: f64,
}

/// Writes one side's WAV over (end, duration), or None when that side has nothing.
type WavWriter<'a> = dyn FnMut(Kind, f64, f64) -> Option<PathBuf> + 'a;

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
    source: &Source,
    wav: &mut WavWriter,
    ffmpeg: &Path,
    clip_seconds: u32,
    folder: &Path,
    save_time: f64,
    id: &str,
) -> Result<Job, GiveUp> {
    let started = Instant::now();
    let dir = source.dir;
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
            (name.starts_with("seg_") && name.ends_with(".ts")).then(|| Segment {
                name,
                mtime: clock::seconds(mtime),
            })
        })
        .collect();
    let listed = match source.clock_zero {
        Some(zero) => selection::parse_segment_list(
            &std::fs::read_to_string(dir.join(SEGMENT_LIST_FILE)).unwrap_or_default(),
            zero,
        ),
        None => Default::default(),
    };
    let age = save_time - source.started_at;
    let chosen = selection::select(&segments, &listed, save_time, clip_seconds, age).inspect_err(|r| {
        say(format!(
            "ABORT: {r:?} ({} segments, capture started {age:.1}s ago)",
            segments.len()
        ))
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
        if let Err(e) = crate::winutil::copy_new(&dir.join(&s.name), &snap) {
            say(format!("ABORT: snapshot of {} failed: {e}", s.name));
            snapshots.iter().for_each(|p| drop(std::fs::remove_file(p)));
            return Err(GiveUp::Error);
        }
        snapshots.push(snap);
    }
    let list = dir.join(format!("concat_{id}.txt"));
    let created = crate::winutil::create_new(&list);
    let listed = created.is_ok();
    let written =
        created.and_then(|mut f| std::io::Write::write_all(&mut f, ffmpeg_cmd::concat_list(&snapshots).as_bytes()));
    // The clip's name is reserved now: two saves in the same second each get their own.
    let output = written
        .and_then(|_| std::fs::create_dir_all(folder))
        .and_then(|_| reserve_output(folder, &local_stamp()));
    let Ok(output) = output else {
        say("ABORT: concat list, output folder or clip name not writable".into());
        snapshots.iter().for_each(|p| drop(std::fs::remove_file(p)));
        // Only what this save created: a list that already existed is not ours.
        if listed {
            let _ = std::fs::remove_file(&list);
        }
        return Err(GiveUp::Error);
    };

    // The audio window is the video's: same end, same length as the concat's -t.
    let loopback = wav(Kind::Loopback, save_time, chosen.duration);
    let mic = wav(Kind::Mic, save_time, chosen.duration);
    Ok(Job {
        id: id.into(),
        ffmpeg: ffmpeg.into(),
        duration: chosen.duration,
        list,
        snapshots,
        loopback,
        mic,
        work: dir.into(),
        output,
        started,
    })
}

/// `Clip_<stamp>.mp4`, else `Clip_<stamp>_2.mp4` and on, created empty so no other save can take
/// it: two saves in the same second once shared a name, and the second (ffmpeg -y) overwrote
/// the first's clip, or deleted it on failure.
fn reserve_output(folder: &Path, stamp: &str) -> std::io::Result<PathBuf> {
    // Bounded: an error that is not "taken" ends it, and so do a thousand taken names.
    for n in 1..=1000 {
        let name = if n == 1 {
            format!("Clip_{stamp}.mp4")
        } else {
            format!("Clip_{stamp}_{n}.mp4")
        };
        let path = folder.join(name);
        match crate::winutil::create_new(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::ErrorKind::AlreadyExists.into())
}

/// Whether the step that writes the clip succeeded: the mux with audio (which only runs after
/// a successful concat), the concat without.
fn written(has_audio: bool, concat: Option<bool>, mux: Option<bool>) -> bool {
    (if has_audio { mux } else { concat }) == Some(true)
}

/// The audio track to mux. Both sides: the mix, but a failed or timed-out mix keeps the game
/// sound rather than losing the clip, and a mixed WAV that is not there is never trusted.
fn pick_audio(loopback: &Path, mixed: &Path, mix_ok: bool, mixed_exists: bool) -> PathBuf {
    if mix_ok && mixed_exists {
        mixed.into()
    } else {
        loopback.into()
    }
}

impl Job {
    fn say(&self, msg: &str) {
        log(&format!("[{}] {msg}", self.id));
    }

    fn run(self) -> Outcome {
        let has_audio = self.loopback.is_some() || self.mic.is_some();
        let video = if has_audio {
            self.work.join(format!("video_{}.mp4", self.id))
        } else {
            self.output.clone()
        };
        // Two saves in the same second share the output name: only the run whose ffmpeg
        // actually wrote it may judge or delete it.
        let concat = self.ffmpeg("concat", &ffmpeg_cmd::concat_args(&self.list, self.duration, &video));
        let mut mux = None;
        let mut temp = vec![self.list.clone()];
        if has_audio && concat == Some(true) {
            let audio = match (&self.loopback, &self.mic) {
                (Some(lb), Some(mic)) => {
                    let mixed = self.work.join(format!("mixed_{}.wav", self.id));
                    temp.push(mixed.clone());
                    let mix_ok = self.ffmpeg("mix", &ffmpeg_cmd::mix_args(lb, mic, &mixed)) == Some(true);
                    pick_audio(lb, &mixed, mix_ok, mixed.is_file())
                }
                (Some(only), None) | (None, Some(only)) => only.clone(),
                (None, None) => unreachable!(),
            };
            mux = self.ffmpeg("mux", &ffmpeg_cmd::mux_args(&video, &audio, &self.output));
        }
        // The name was reserved for this save: a clip that did not come out is removed.
        let saved = written(has_audio, concat, mux) && std::fs::metadata(&self.output).is_ok_and(|m| m.len() > 0);
        temp.extend(self.snapshots.iter().cloned());
        temp.extend(self.loopback.iter().chain(self.mic.iter()).cloned());
        if has_audio {
            temp.push(video);
        }
        temp.iter().for_each(|p| drop(std::fs::remove_file(p)));
        if !saved {
            let _ = std::fs::remove_file(&self.output);
        }
        self.say(&format!(
            "done: saved={saved} output={} took={:.1}s",
            self.output.display(),
            self.started.elapsed().as_secs_f64()
        ));
        if saved {
            Ok(self.duration.round() as u32)
        } else {
            Err(GiveUp::Error)
        }
    }

    /// Runs one ffmpeg with a timeout; logs its exit and, on failure, its stderr's tail.
    /// None: it never started (so it wrote nothing). Some(ok) otherwise.
    fn ffmpeg(&self, what: &str, args: &[String]) -> Option<bool> {
        let child = Command::new(&self.ffmpeg)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            self.say(&format!("{what}: ffmpeg did not start"));
            return None;
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
            let tail: String = text
                .chars()
                .rev()
                .take(2000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            self.say(&format!(
                "{what}: FAILED ({}): {tail}",
                status.map_or("timed out".into(), |s| s.to_string())
            ));
        }
        Some(ok)
    }
}

/// Local time on purpose: the clip name is what the user reads in Explorer, like ShadowPlay's.
/// What is stored for the app (the log) is UTC.
fn local_stamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crtest-save-{name}-{}", crate::winutil::random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Six 1 s segments with mtimes 1 s apart, the newest closing now (it is still being written).
    fn segments(dir: &Path, now: f64) {
        for i in 0..6 {
            let path = dir.join(format!("seg_{i:03}.ts"));
            std::fs::write(&path, format!("segment {i}")).unwrap();
            let mtime = UNIX_EPOCH + Duration::from_secs_f64(now - (5 - i) as f64);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
        }
    }

    fn now() -> f64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
    }

    fn no_audio() -> impl FnMut(Kind, f64, f64) -> Option<PathBuf> {
        |_, _, _| None
    }

    fn try_prepare(dir: &Path, started_at: f64, save_time: f64, wav: &mut WavWriter) -> Result<Job, GiveUp> {
        let source = Source {
            dir,
            clock_zero: None,
            started_at,
        };
        prepare(
            &source,
            wav,
            Path::new("ffmpeg.exe"),
            3,
            &dir.join("clips"),
            save_time,
            "t",
        )
    }

    /// Only seg_*.ts files are segments: a stray file never becomes part of a clip, and an
    /// output folder that cannot be made is an error.
    #[test]
    fn only_segment_files_are_snapshotted_and_an_unwritable_folder_is_an_error() {
        let dir = temp("names");
        let t = now();
        segments(&dir, t);
        for stray in ["seg_000.txt", "notes.ts", "x_seg_001.ts"] {
            std::fs::write(dir.join(stray), "stray").unwrap();
            let old = UNIX_EPOCH + Duration::from_secs_f64(t - 2.5);
            std::fs::File::options()
                .write(true)
                .open(dir.join(stray))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let job = try_prepare(&dir, t - 60.0, t, &mut no_audio()).unwrap();
        for snap in &job.snapshots {
            assert_ne!(std::fs::read_to_string(snap).unwrap(), "stray");
        }
        std::fs::write(dir.join("blocked"), "a file where the folder should be").unwrap();
        let blocked = Source {
            dir: &dir,
            clock_zero: None,
            started_at: t - 60.0,
        };
        let result = prepare(
            &blocked,
            &mut no_audio(),
            Path::new("ffmpeg.exe"),
            3,
            &dir.join("blocked").join("sub"),
            t,
            "u",
        );
        assert!(matches!(result, Err(GiveUp::Error)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_segment_folder_that_cannot_be_listed_is_an_error() {
        let missing = std::env::temp_dir().join(format!("crtest-save-missing-{}", crate::winutil::random_hex()));
        assert!(matches!(
            try_prepare(&missing, 0.0, now(), &mut no_audio()),
            Err(GiveUp::Error)
        ));
    }

    #[test]
    fn a_press_right_after_the_capture_started_is_too_soon_not_an_error() {
        let dir = temp("soon");
        let t = now();
        assert!(matches!(
            try_prepare(&dir, t - 1.0, t, &mut no_audio()),
            Err(GiveUp::TooSoon)
        ));
        assert!(matches!(
            try_prepare(&dir, t - 30.0, t, &mut no_audio()),
            Err(GiveUp::Error)
        ));
    }

    /// The concat reads only copies made at the press (the live ffmpeg rewrites its own names
    /// under -segment_wrap), never the segment still being written, and the audio window is
    /// exactly the concat's: same end, same length.
    #[test]
    fn the_concat_reads_only_snapshots_and_the_audio_window_is_the_videos() {
        let dir = temp("snap");
        let t = now();
        segments(&dir, t);
        let mut calls = Vec::new();
        let mut wav = |kind: Kind, end: f64, duration: f64| {
            calls.push((kind, end, duration));
            None
        };
        let job = try_prepare(&dir, t - 60.0, t, &mut wav).unwrap();
        let list = std::fs::read_to_string(&job.list).unwrap();
        assert!(!list.is_empty());
        for line in list.lines() {
            assert!(line.contains("/snap_t_"), "{line}");
        }
        let copied: Vec<String> = job
            .snapshots
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        assert!(
            !copied.contains(&"segment 5".to_string()),
            "the in-progress segment was copied: {copied:?}"
        );
        assert_eq!(calls, [(Kind::Loopback, t, job.duration), (Kind::Mic, t, job.duration)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_snapshot_that_fails_is_an_error_and_leaves_no_copy_behind() {
        let dir = temp("copyfail");
        let t = now();
        segments(&dir, t);
        // A segment held open with no sharing, as an antivirus scan can: listed and dated, but
        // it cannot be read while the others can.
        use std::os::windows::fs::OpenOptionsExt;
        let lock = std::fs::File::options()
            .read(true)
            .share_mode(0)
            .open(dir.join("seg_004.ts"))
            .unwrap();
        let result = try_prepare(&dir, t - 60.0, t, &mut no_audio());
        drop(lock);
        assert!(matches!(result, Err(GiveUp::Error)));
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.iter().all(|n| !n.starts_with("snap_")), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_clip_is_judged_by_the_step_that_writes_it() {
        assert!(written(false, Some(true), None), "no audio: the concat writes it");
        assert!(!written(false, Some(false), None));
        assert!(!written(false, None, None), "ffmpeg never started");
        assert!(written(true, Some(true), Some(true)), "audio: the mux writes it");
        assert!(!written(true, Some(true), Some(false)));
        assert!(!written(true, Some(false), None), "a failed concat means no mux");
        assert!(!written(true, Some(true), None));
    }

    /// Two saves in the same second: each gets its own clip, neither can overwrite the other.
    #[test]
    fn two_saves_in_the_same_second_get_their_own_clip() {
        let dir = temp("names2");
        let first = reserve_output(&dir, "20261113_101500").unwrap();
        let second = reserve_output(&dir, "20261113_101500").unwrap();
        assert_eq!(first.file_name().unwrap(), "Clip_20261113_101500.mp4");
        assert_eq!(second.file_name().unwrap(), "Clip_20261113_101500_2.mp4");
        let missing = reserve_output(&dir.join("no such folder"), "20261113_101500").unwrap_err();
        assert_eq!(
            missing.kind(),
            std::io::ErrorKind::NotFound,
            "only a taken name moves on"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file already sitting under a snapshot's name aborts the save, which leaves it alone.
    #[test]
    fn a_save_never_deletes_a_file_it_did_not_create() {
        let dir = temp("planted");
        let t = now();
        segments(&dir, t);
        let planted = dir.join("snap_t_001.ts");
        std::fs::write(&planted, "not this save's").unwrap();
        assert!(matches!(
            try_prepare(&dir, t - 60.0, t, &mut no_audio()),
            Err(GiveUp::Error)
        ));
        assert_eq!(std::fs::read_to_string(&planted).unwrap(), "not this save's");
        assert!(
            !dir.join("snap_t_000.ts").exists(),
            "this save's own copy was left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_mix_keeps_the_game_sound_and_a_missing_mix_is_never_trusted() {
        let (lb, mixed) = (Path::new("lb.wav"), Path::new("mixed.wav"));
        assert_eq!(pick_audio(lb, mixed, true, true), mixed);
        assert_eq!(pick_audio(lb, mixed, false, true), lb, "failed or timed out");
        assert_eq!(pick_audio(lb, mixed, true, false), lb, "no file written");
    }

    fn job(dir: &Path, output: PathBuf, audio: bool) -> Job {
        let wav = dir.join("loopback_t.wav");
        std::fs::write(&wav, b"RIFF").unwrap();
        Job {
            id: "t".into(),
            ffmpeg: dir.join("no-such-ffmpeg.exe"),
            duration: 3.0,
            list: dir.join("concat_t.txt"),
            snapshots: vec![],
            loopback: audio.then_some(wav),
            mic: None,
            work: dir.into(),
            output,
            started: Instant::now(),
        }
    }

    /// A failed encode is reported, never silent, and leaves no empty clip under its reserved name.
    #[test]
    fn a_failed_encode_reports_and_leaves_no_empty_clip() {
        for audio in [true, false] {
            let dir = temp("encode");
            let output = reserve_output(&dir, "t").unwrap();
            assert!(matches!(job(&dir, output.clone(), audio).run(), Err(GiveUp::Error)));
            assert!(!output.exists(), "audio={audio}: an empty clip was left");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// examples/fake_ffmpeg.exe, copied into `dir` under `name` (its name picks its behaviour).
    fn fake_ffmpeg(dir: &Path, name: &str) -> PathBuf {
        // Built under target/<profile>/examples with a hash in its name.
        let exe = std::env::current_exe().unwrap();
        let examples = exe.parent().unwrap().parent().unwrap().join("examples");
        let built = std::fs::read_dir(&examples)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                name.starts_with("fake_ffmpeg") && name.ends_with(".exe")
            })
            .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
            .expect("run the tests with --examples: no fake_ffmpeg built");
        let fake = dir.join(format!("{name}.exe"));
        std::fs::copy(built, &fake).unwrap();
        fake
    }

    /// A job over two segments' worth of work, with the sides asked for, run by a fake ffmpeg.
    fn run_with(fake: &str, loopback: bool, mic: bool) -> (Outcome, PathBuf, PathBuf) {
        let dir = temp(fake);
        let wav = |side: &str| {
            let path = dir.join(format!("{side}_t.wav"));
            std::fs::write(&path, b"RIFF").unwrap();
            path
        };
        let list = dir.join("concat_t.txt");
        std::fs::write(&list, "file 'snap'").unwrap();
        let snapshot = dir.join("snap_t_000.ts");
        std::fs::write(&snapshot, "segment").unwrap();
        let output = dir.join("Clip_t.mp4");
        let job = Job {
            id: "t".into(),
            ffmpeg: fake_ffmpeg(&dir, fake),
            duration: 3.4,
            list,
            snapshots: vec![snapshot],
            loopback: loopback.then(|| wav("loopback")),
            mic: mic.then(|| wav("mic")),
            work: dir.clone(),
            output: output.clone(),
            started: Instant::now(),
        };
        (job.run(), output, dir)
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".exe") && n != "Clip_t.mp4")
            .collect()
    }

    /// Without audio the concat writes the clip; with audio it is muxed with the mix of both
    /// sides, or the one side there is. Every temporary file goes, the clip stays.
    #[test]
    fn a_save_muxes_the_right_audio_and_cleans_up_after_itself() {
        for (loopback, mic, expected) in [
            (false, false, "concat: "),
            (true, true, "mixed_t.wav"),
            (true, false, "loopback_t.wav"),
            (false, true, "mic_t.wav"),
        ] {
            let (outcome, output, dir) = run_with("ffmpeg", loopback, mic);
            assert_eq!(outcome, Ok(3), "loopback={loopback} mic={mic}");
            let clip = std::fs::read_to_string(&output).unwrap();
            assert!(clip.contains(expected), "loopback={loopback} mic={mic}: {clip}");
            assert_eq!(leftovers(&dir), Vec::<String>::new(), "temporary files left behind");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_failed_mix_still_saves_the_clip_with_the_game_sound() {
        let (outcome, output, dir) = run_with("ffmpeg-failmix", true, true);
        assert_eq!(outcome, Ok(3));
        let clip = std::fs::read_to_string(&output).unwrap();
        assert!(
            clip.contains("loopback_t.wav") && !clip.contains("mixed_t.wav"),
            "{clip}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clip this run wrote but that failed, or came out empty, is reported and removed.
    #[test]
    fn a_failed_or_empty_clip_is_reported_and_removed() {
        for (fake, loopback) in [("ffmpeg-failmux", true), ("ffmpeg-empty", false)] {
            let (outcome, output, dir) = run_with(fake, loopback, false);
            assert_eq!(outcome, Err(GiveUp::Error), "{fake}");
            assert!(!output.exists(), "{fake}: the broken clip was kept");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
