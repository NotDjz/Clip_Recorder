//! Startup backstop for ffmpegs a previous run left alive (killed before the job object existed,
//! or by something the job cannot see). An orphan keeps writing to %TEMP% and holds an NVENC
//! session; consumer GPUs allow only a few, so enough orphans make every later launch encode
//! nothing at all.

use crate::logging::log;
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

/// Prefix of every capture's segment folder in %TEMP%: written by capture.rs, scanned here.
pub const SEGMENT_DIR_PREFIX: &str = "cliprec_";
pub const PID_FILE: &str = "ffmpeg.pid";
/// One budget for the whole sweep, so many orphans cannot stall the launch.
const SWEEP_BUDGET: Duration = Duration::from_millis(3000);
/// Never wait zero: the folder removal that follows would race the still-open segment file.
const WAIT_FLOOR: Duration = Duration::from_millis(250);

/// Kills each leftover capture ffmpeg, then removes its folder. One failure never stops the rest.
pub fn sweep(temp: &Path) {
    let Ok(entries) = std::fs::read_dir(temp) else { return };
    let deadline = Instant::now() + SWEEP_BUDGET;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(SEGMENT_DIR_PREFIX) {
            continue;
        }
        let dir = entry.path();
        kill_orphan(&dir, deadline);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Terminates the process in `dir`'s PID file, but only if it still is an ffmpeg.exe: PIDs are
/// reused, and killing a stranger's process is worse than leaving an orphan.
pub fn kill_orphan(dir: &Path, deadline: Instant) {
    let Some(pid) = std::fs::read_to_string(dir.join(PID_FILE))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return;
    };
    unsafe {
        // SYNCHRONIZE, or the wait below silently does nothing.
        let h = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if h.is_null() {
            return;
        }
        let mut buf = [0u16; 32768];
        let mut size = buf.len() as u32;
        if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut size) != 0 {
            let image = String::from_utf16_lossy(&buf[..size as usize]);
            let name = Path::new(&image)
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase());
            if name.as_deref() == Some("ffmpeg.exe") {
                TerminateProcess(h, 1);
                // TerminateProcess is asynchronous.
                let wait = deadline.saturating_duration_since(Instant::now()).max(WAIT_FLOOR);
                WaitForSingleObject(h, wait.as_millis() as u32);
                log(&format!("killed orphan ffmpeg pid={pid} from {}", dir.display()));
            }
        }
        CloseHandle(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        // Never cliprec_*: a running Clip Recorder would sweep it away.
        let dir = std::env::temp_dir().join(format!("crtest_orphan_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_reused_pid_that_is_not_ffmpeg_is_never_killed() {
        let dir = temp_dir("foreign");
        let mut stranger = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(dir.join(PID_FILE), stranger.id().to_string()).unwrap();
        kill_orphan(&dir, Instant::now());
        let alive = stranger.try_wait().unwrap().is_none();
        let _ = stranger.kill();
        let _ = stranger.wait();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(alive, "a process that was not ffmpeg.exe got killed");
    }

    #[test]
    fn a_missing_or_garbage_pid_file_is_tolerated() {
        let dir = temp_dir("garbage");
        kill_orphan(&dir, Instant::now());
        std::fs::write(dir.join(PID_FILE), "not a pid").unwrap();
        kill_orphan(&dir, Instant::now());
        std::fs::write(dir.join(PID_FILE), "4000000000").unwrap();
        kill_orphan(&dir, Instant::now());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn ping() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
            .join(r"System32\ping.exe")
    }

    /// The kill path itself: an orphan named ffmpeg.exe is terminated, and the sweep waits for
    /// it to be gone (SYNCHRONIZE in the access mask) before removing its folder.
    #[test]
    fn an_orphan_ffmpeg_is_killed_and_waited_for() {
        let dir = temp_dir("orphan");
        let fake = dir.join("ffmpeg.exe");
        std::fs::copy(ping(), &fake).unwrap();
        let mut orphan = std::process::Command::new(&fake)
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(dir.join(PID_FILE), orphan.id().to_string()).unwrap();
        kill_orphan(&dir, Instant::now());
        let gone = orphan.try_wait().unwrap().is_some();
        if !gone {
            let _ = orphan.kill();
        }
        let _ = orphan.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(gone, "the orphan ffmpeg was still running after kill_orphan returned");
    }

    /// Each cliprec_ folder is handled on its own (one with a garbage PID file does not stop
    /// the next), and nothing else in the folder is touched.
    #[test]
    fn the_sweep_clears_every_capture_folder_and_nothing_else() {
        let root = temp_dir("sweep");
        for (name, pid) in [("cliprec_1_a", "garbage"), ("cliprec_2_b", "4000000000")] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join(PID_FILE), pid).unwrap();
            std::fs::write(root.join(name).join("seg_000.ts"), "x").unwrap();
        }
        std::fs::create_dir_all(root.join("someone_else")).unwrap();
        std::fs::write(root.join("cliprec_not_a_folder.txt"), "x").unwrap();
        sweep(&root);
        assert!(!root.join("cliprec_1_a").exists() && !root.join("cliprec_2_b").exists());
        assert!(
            root.join("someone_else").exists(),
            "a folder that is not a capture's was removed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
