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
