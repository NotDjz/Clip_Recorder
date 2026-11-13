//! The capture ffmpeg: started, watched, restarted, stopped. It writes 1 s MPEG-TS segments in a
//! %TEMP%\cliprec_* folder that save.rs picks clips from.

use crate::logging::log;
use crate::monitors::Screen;
use crate::orphans::{PID_FILE, SEGMENT_DIR_PREFIX};
use clip_recorder::clock::{self, Estimator, capture_pts};
use clip_recorder::config::Config;
use clip_recorder::ffmpeg_cmd::{self, Encoder, Grabber};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// -debug_ts writes ~1.5 KB a frame, and ffmpeg stalls, capture included, whenever its stderr
/// pipe is full: the default 4 KB gave the reader ~11 ms at 240 fps. 1 MB is ~2.8 s.
const STDERR_PIPE_BYTES: u32 = 1 << 20;

/// ffmpeg.exe beside our exe, where the MSI puts it. CLIPREC_FFMPEG overrides it (tests, dev).
pub fn ffmpeg_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CLIPREC_FFMPEG").filter(|p| !p.is_empty()) {
        return p.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("ffmpeg.exe")))
        .unwrap_or_else(|| "ffmpeg.exe".into())
}

/// Whether ffmpeg really encodes one frame from `input`. Listing h264_nvenc or ddagrab only says
/// they were compiled in: without an NVIDIA GPU both are listed and fail, and the capture died
/// and restarted every two seconds (seen in Windows Sandbox).
fn works(ffmpeg: &Path, input: &str, encoder: &str) -> bool {
    let child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            input,
            "-frames:v",
            "1",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// DXGI capture straight into NVENC when both work. ddagrab hands over GPU frames, which x264
/// cannot read, so without NVENC the capture goes through GDI (an AMD or Intel GPU today).
fn pipeline(ffmpeg: &Path) -> (Encoder, Grabber) {
    let dda = works(ffmpeg, "ddagrab=output_idx=0", "h264_nvenc");
    ffmpeg_cmd::choose(dda, dda || works(ffmpeg, "color=c=black:s=256x256:d=1", "h264_nvenc"))
}

/// One running ffmpeg with its own capture clock: a reader left over from a replaced process
/// can only ever write to its own.
struct Running {
    child: Child,
    clock: Arc<Mutex<Option<f64>>>,
    /// The last lines ffmpeg printed that were not -debug_ts traffic: why it died.
    last_words: Arc<Mutex<VecDeque<String>>>,
}

const LAST_WORDS: usize = 6;

pub struct Capture {
    ffmpeg: PathBuf,
    segment_dir: PathBuf,
    encoder: Encoder,
    grabber: Grabber,
    running: Option<Running>,
    /// Started and not stopped: a capture that failed to spawn still counts as one to restart.
    wanted: bool,
    started_at: f64,
}

impl Capture {
    /// Joins the kill-on-close job first: before any ffmpeg exists, the probes included.
    pub fn new(ffmpeg: PathBuf) -> std::io::Result<Capture> {
        Capture::new_in(&clip_recorder::paths::segments_root(), ffmpeg)
    }

    fn new_in(root: &Path, ffmpeg: PathBuf) -> std::io::Result<Capture> {
        crate::job::ensure_kill_on_close();
        let segment_dir = root.join(format!(
            "{SEGMENT_DIR_PREFIX}{}_{}",
            std::process::id(),
            crate::winutil::random_hex()
        ));
        std::fs::create_dir_all(&segment_dir)?;
        let (encoder, grabber) = pipeline(&ffmpeg);
        log(&format!(
            "capture: encoder={encoder:?} grabber={grabber:?} dir={}",
            segment_dir.display()
        ));
        Ok(Capture {
            ffmpeg,
            segment_dir,
            encoder,
            grabber,
            running: None,
            wanted: false,
            started_at: 0.0,
        })
    }

    /// Probes again when launch settled for less than DXGI + NVENC: at sign-in the driver or
    /// DXGI may not be ready yet, and one failed probe must not leave the whole session on GDI.
    /// True when the pipeline improved (the caller restarts the video).
    pub fn reprobe(&mut self) -> bool {
        if (self.encoder, self.grabber) == (Encoder::Nvenc, Grabber::Ddagrab) {
            return false;
        }
        let (encoder, grabber) = pipeline(&self.ffmpeg);
        let better = ffmpeg_cmd::better((self.encoder, self.grabber), (encoder, grabber));
        if better {
            log(&format!(
                "capture: pipeline now encoder={encoder:?} grabber={grabber:?}"
            ));
            (self.encoder, self.grabber) = (encoder, grabber);
        }
        better
    }

    pub fn segment_dir(&self) -> &Path {
        &self.segment_dir
    }

    pub fn encoder(&self) -> Encoder {
        self.encoder
    }

    pub fn grabber(&self) -> Grabber {
        self.grabber
    }

    /// Wall-clock time the current ffmpeg started: tells "pressed too soon" from "broken".
    pub fn started_at(&self) -> f64 {
        self.started_at
    }

    /// The capture clock's zero, once the current ffmpeg has printed enough frames.
    pub fn clock_zero(&self) -> Option<f64> {
        self.running
            .as_ref()
            .and_then(|r| *r.clock.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn is_running(&mut self) -> bool {
        self.running
            .as_mut()
            .is_some_and(|r| matches!(r.child.try_wait(), Ok(None)))
    }

    /// True if ffmpeg had been started and has since died (the health check restarts it).
    pub fn has_died(&mut self) -> bool {
        let died = self.wanted && !self.is_running();
        if died && let Some(r) = &self.running {
            let words = r.last_words.lock().unwrap_or_else(|e| e.into_inner());
            log(&format!(
                "capture ffmpeg exited ({:?}): {}",
                r.child.id(),
                words.iter().cloned().collect::<Vec<_>>().join(" | ")
            ));
        }
        died
    }

    pub fn start(&mut self, config: &Config, screens: &[Screen]) {
        if self.is_running() {
            return;
        }
        self.wanted = true;
        self.started_at = clock::now();
        let _ = std::fs::create_dir_all(&self.segment_dir);
        // A restart starts from an empty set: mixing a dead capture's segments (its last one
        // cut mid-write) with the new one put the restart gap inside a clip.
        self.wipe_segments();
        let index = config.monitor.min(screens.len().saturating_sub(1));
        let args = ffmpeg_cmd::capture_args(&ffmpeg_cmd::Capture {
            fps: config.fps,
            buffer_seconds: config.buffer_seconds,
            monitor_index: index,
            monitor: screens[index].rect,
            grabber: self.grabber,
            encoder: self.encoder,
            segment_dir: &self.segment_dir,
        });
        match self.spawn(&args) {
            Ok(running) => {
                let pid = running.child.id();
                // A later launch kills this pid if we die before stop() runs and the job missed it.
                let _ = std::fs::write(self.segment_dir.join(PID_FILE), pid.to_string());
                log(&format!(
                    "capture started: pid={pid} in_job={} fps={} buffer={} monitor={index}",
                    crate::job::contains(&running.child),
                    ffmpeg_cmd::effective_fps(config.fps, self.grabber),
                    config.buffer_seconds
                ));
                self.running = Some(running);
            }
            Err(e) => {
                log(&format!("capture failed to start: {e}"));
                self.running = None;
            }
        }
    }

    fn spawn(&self, args: &[String]) -> std::io::Result<Running> {
        let (read, write) = stderr_pipe()?;
        let child = Command::new(&self.ffmpeg)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(write))
            .spawn()?;
        // The Command, and our copy of the write end with it, is gone here: otherwise the
        // reader would never see EOF.
        let clock = Arc::new(Mutex::new(None));
        let last_words = Arc::new(Mutex::new(VecDeque::new()));
        let (published, said) = (Arc::clone(&clock), Arc::clone(&last_words));
        std::thread::spawn(move || read_clock(read, published, said));
        Ok(Running {
            child,
            clock,
            last_words,
        })
    }

    /// Stops ffmpeg cleanly ("q" lets it close the segment), then harder.
    pub fn stop(&mut self) {
        self.wanted = false;
        let Some(mut running) = self.running.take() else { return };
        if let Some(stdin) = running.child.stdin.as_mut() {
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if !matches!(running.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = running.child.kill();
        let _ = running.child.wait();
    }

    /// Monitor, fps or buffer changed: the video process only. Restarting audio needlessly
    /// re-exposed the flaky loopback reopen that caused a desync.
    pub fn restart_video(&mut self, config: &Config, screens: &[Screen]) {
        self.stop();
        self.start(config, screens);
    }

    /// Only the live capture's files. Never a running save's snapshots, lists, WAVs or video,
    /// and never the PID file.
    fn wipe_segments(&self) {
        let Ok(entries) = std::fs::read_dir(&self.segment_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if ffmpeg_cmd::is_capture_file(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    pub fn cleanup(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.segment_dir);
    }
}

fn stderr_pipe() -> std::io::Result<(std::fs::File, OwnedHandle)> {
    use windows_sys::Win32::System::Pipes::CreatePipe;
    let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), STDERR_PIPE_BYTES) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe {
        Ok((
            std::fs::File::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        ))
    }
}

/// Drains ffmpeg's stderr to EOF (a full pipe would stall the capture) and publishes the capture
/// clock as each window closes.
fn read_clock(stderr: std::fs::File, published: Arc<Mutex<Option<f64>>>, last_words: Arc<Mutex<VecDeque<String>>>) {
    let mut estimator = Estimator::default();
    for line in BufReader::new(stderr).split(b'\n') {
        let Ok(line) = line else { break };
        if let Some(pts) = capture_pts(&line) {
            if let Some(zero) = estimator.observe(clock::now(), pts) {
                *published.lock().unwrap_or_else(|e| e.into_inner()) = Some(zero);
            }
            continue;
        }
        let text = String::from_utf8_lossy(&line).trim().to_string();
        // -debug_ts prints every packet with an arrow; progress lines start with frame=.
        if text.is_empty() || text.contains(" -> ") || text.contains(" <- ") || text.starts_with("frame=") {
            continue;
        }
        let mut words = last_words.lock().unwrap_or_else(|e| e.into_inner());
        if words.len() == LAST_WORDS {
            words.pop_front();
        }
        words.push_back(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_recorder::ffmpeg_cmd::{Monitor, SEGMENT_LIST_FILE};

    #[test]
    fn stderr_gets_a_pipe_large_enough_for_debug_ts() {
        const {
            assert!(
                STDERR_PIPE_BYTES >= 1 << 20,
                "a small pipe stalls the capture at 240 fps"
            )
        };
    }

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("crtest-capture-{}", crate::winutil::random_hex()))
    }

    fn screens() -> Vec<Screen> {
        vec![Screen {
            name: String::new(),
            rect: Monitor {
                x: 0,
                y: 0,
                w: 640,
                h: 480,
            },
            primary: true,
        }]
    }

    /// ping.exe stands in for ffmpeg: it starts, refuses ffmpeg's arguments and exits.
    fn fake_ffmpeg() -> PathBuf {
        PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\ping.exe")
    }

    #[test]
    fn without_a_working_encoder_the_capture_falls_back_to_x264_through_gdi() {
        let root = root();
        let capture = Capture::new_in(&root, fake_ffmpeg()).unwrap();
        assert_eq!(
            (capture.encoder(), capture.grabber()),
            (Encoder::X264, Grabber::Gdigrab)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The capture records its ffmpeg's pid (a later launch kills it if the job missed it), a
    /// restart wipes the dead capture's segments and list but not a save's files, and a capture
    /// that died is reported for restart.
    #[test]
    fn a_start_records_the_pid_wipes_the_old_segments_and_a_death_is_noticed() {
        let root = root();
        let mut capture = Capture::new_in(&root, fake_ffmpeg()).unwrap();
        let dir = capture.segment_dir().to_path_buf();
        for name in ["seg_000.ts", SEGMENT_LIST_FILE, "snap_ab_000.ts", "loopback_ab.wav"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        capture.start(&Config::default(), &screens());
        let pid: u32 = std::fs::read_to_string(dir.join(PID_FILE))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(Some(pid), capture.running.as_ref().map(|r| r.child.id()));
        assert!(!dir.join("seg_000.ts").exists() && !dir.join(SEGMENT_LIST_FILE).exists());
        assert!(
            dir.join("snap_ab_000.ts").exists() && dir.join("loopback_ab.wav").exists(),
            "a save's files were wiped"
        );
        let died = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            capture.has_died()
        });
        assert!(died, "a dead capture was not noticed");
        capture.cleanup();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A capture whose ffmpeg could not even be started is still one to restart: has_died used
    /// to need a process, so a failed spawn stopped the capture for good.
    #[test]
    fn a_capture_that_failed_to_spawn_is_retried() {
        let root = root();
        let mut capture = Capture::new_in(&root, root.join("no-such-ffmpeg.exe")).unwrap();
        capture.start(&Config::default(), &screens());
        assert!(capture.has_died());
        capture.stop();
        assert!(!capture.has_died(), "a stopped capture is not a dead one");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// stderr is drained to its end: the capture clock comes from -debug_ts capture lines, and
    /// the last other lines are kept to say why ffmpeg died.
    #[test]
    fn the_stderr_reader_publishes_the_clock_and_keeps_the_last_words() {
        let (read, write) = stderr_pipe().unwrap();
        // Capture lines spread over more than the first 1 s window, as ffmpeg prints them.
        let writer = std::thread::spawn(move || {
            let mut out = std::fs::File::from(write);
            for i in 0..30 {
                writeln!(out, "[vist#0:0/wrapped_avframe] demuxer+ffmpeg -> ist_index:0:0 type:video pkt_pts:{i} pkt_pts_time:{} pkt_dts:{i}", i as f64 * 0.05).unwrap();
                std::thread::sleep(Duration::from_millis(50));
            }
            // -debug_ts traffic in both directions and the progress line are not last words.
            writeln!(out, "decoder -> ist_index:0:0 type:video frame_pts:1").unwrap();
            writeln!(out, "muxer <- type:video pkt_pts:1").unwrap();
            writeln!(out, "frame=   10 fps=0.0 q=20.0 size=N/A").unwrap();
            writeln!(out).unwrap();
            for i in 0..8 {
                writeln!(out, "Error {i}").unwrap();
            }
        });
        let clock = Arc::new(Mutex::new(None));
        let words = Arc::new(Mutex::new(VecDeque::new()));
        read_clock(read, Arc::clone(&clock), Arc::clone(&words));
        writer.join().unwrap();
        let zero = clock.lock().unwrap().expect("no capture clock published");
        assert!((clock::now() - zero).abs() < 10.0, "zero={zero}");
        let words: Vec<String> = words.lock().unwrap().iter().cloned().collect();
        assert_eq!(
            words,
            ["Error 2", "Error 3", "Error 4", "Error 5", "Error 6", "Error 7"],
            "the last six"
        );
    }
}
