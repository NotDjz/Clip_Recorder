//! The kill-on-close job object: the primary defence against orphaned ffmpegs.
//!
//! This process joins a job that kills every process in it when its last handle closes, which
//! happens when we die by any means: Task Manager, a crash, TerminateProcess. Children inherit
//! the membership, so it covers the capture ffmpeg and every save ffmpeg too. Startup cleanup
//! (orphans.rs) is only the backstop.

use crate::logging::log;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Kill-on-close, and breakaway allowed only on request (never SILENT_BREAKAWAY_OK: the
/// ffmpegs would escape too and the job would be pointless).
pub const LIMITS: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;

/// Call before spawning any ffmpeg, the probes included. The handle is never closed: closing it
/// would kill the job, us with it. Never fails silently: an inert job once shipped unnoticed.
pub fn ensure_kill_on_close() {
    unsafe {
        let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            log(&format!("job object not created (err={}): orphan cleanup is the only defence", GetLastError()));
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = LIMITS;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            log(&format!("job object not assigned (err={}): orphan cleanup is the only defence", GetLastError()));
            CloseHandle(job);
            return;
        }
        JOB.store(job as isize, std::sync::atomic::Ordering::Relaxed);
        log("job object active: ffmpeg children will die with this process");
    }
}

static JOB: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Whether `child` really is in our job: logged for every capture ffmpeg, so a log shows it.
pub fn contains(child: &std::process::Child) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    let job = JOB.load(std::sync::atomic::Ordering::Relaxed) as HANDLE;
    let mut inside = 0;
    !job.is_null() && unsafe { IsProcessInJob(child.as_raw_handle() as HANDLE, job, &mut inside) } != 0 && inside != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    #[test]
    fn the_job_kills_on_close_and_lets_nothing_escape_silently() {
        use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        assert_ne!(LIMITS & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0);
        assert_eq!(LIMITS & JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, 0, "the ffmpegs would escape the job");
    }

    fn ping(seconds: u32) -> Command {
        let mut c = Command::new("ping");
        c.args(["-n", &seconds.to_string(), "127.0.0.1"]).stdout(Stdio::null());
        c
    }

    /// The process really joins its job: an "active" job with nothing in it once shipped.
    #[test]
    fn children_are_in_the_job() {
        ensure_kill_on_close();
        let mut child = ping(30).spawn().unwrap();
        let inside = contains(&child);
        let _ = child.kill();
        let _ = child.wait();
        assert!(inside);
    }

    /// Run by the next test in a process of its own: joins the job, starts a child, prints its
    /// pid, then waits to be killed.
    #[test]
    #[ignore = "helper process for a_force_killed_parent_takes_its_children_along"]
    fn job_helper() {
        if std::env::var_os("CLIPREC_JOB_HELPER").is_none() {
            return;
        }
        ensure_kill_on_close();
        let child = ping(60).spawn().unwrap();
        println!("child={}", child.id());
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    /// TerminateProcess skips every cleanup: the job alone must take the child along.
    #[test]
    fn a_force_killed_parent_takes_its_children_along() {
        let mut helper = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "job::tests::job_helper", "--ignored", "--nocapture", "--test-threads=1"])
            .env("CLIPREC_JOB_HELPER", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(helper.stdout.take().unwrap());
        let pid: u32 = out.lines().map_while(Result::ok).find_map(|l| l.split("child=").nth(1).and_then(|p| p.trim().parse().ok())).expect("no child pid");
        helper.kill().unwrap();
        let _ = helper.wait();
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
        let died = unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                true
            } else {
                let died = WaitForSingleObject(h, 3000) == WAIT_OBJECT_0;
                if !died {
                    TerminateProcess(h, 1);
                }
                CloseHandle(h);
                died
            }
        };
        assert!(died, "the child outlived its force-killed parent");
    }
}
