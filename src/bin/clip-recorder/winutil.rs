//! Small Win32 helpers shared by the app's modules.

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// 128 random bits from the system CSPRNG, as 32 hex digits: names for temporary files nobody
/// can predict (CWE-378), combined with create_new so none is ever overwritten.
pub fn random_hex() -> String {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let mut bytes = [0u8; 16];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    assert!(status >= 0, "BCryptGenRandom failed: {status:#x}");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new file, never an existing one (AlreadyExists otherwise).
pub fn create_new(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// Copies a file to a new name with the system's own copy, never over an existing file
/// (AlreadyExists then). A byte loop through 8 KiB buffers was slower on the UI thread.
pub fn copy_new(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::CopyFileW;
    let (from, to) = (wide(&from.to_string_lossy()), wide(&to.to_string_lossy()));
    if unsafe { CopyFileW(from.as_ptr(), to.as_ptr(), 1) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Waits for `child` up to `timeout`, checking every `poll`; past it, kills it and returns None.
/// One rule for every short ffmpeg (the pipeline probes, a save's steps), so none can hang.
pub fn wait_or_kill(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
    poll: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(poll),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_utf16_and_nul_terminated() {
        assert_eq!(wide("Aé"), [0x41, 0xE9, 0]);
        assert_eq!(wide(""), [0]);
    }

    #[test]
    fn random_names_are_128_bits_of_hex_and_never_repeat() {
        let (a, b) = (random_hex(), random_hex());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn copy_new_copies_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("crtest-copy-{}", random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, "segment").unwrap();
        copy_new(&a, &b).unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "segment");
        std::fs::write(&a, "newer").unwrap();
        assert_eq!(copy_new(&a, &b).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "segment");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wait_or_kill_returns_the_status_or_kills_on_timeout() {
        use std::time::{Duration, Instant};
        let mut quick = std::process::Command::new("cmd")
            .args(["/c", "exit 3"])
            .spawn()
            .unwrap();
        let status = wait_or_kill(&mut quick, Duration::from_secs(10), Duration::from_millis(10));
        assert_eq!(status.and_then(|s| s.code()), Some(3));
        let mut slow = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = slow.id();
        let start = Instant::now();
        assert_eq!(
            wait_or_kill(&mut slow, Duration::from_millis(200), Duration::from_millis(10)),
            None
        );
        assert!(start.elapsed() < Duration::from_secs(5), "waited past the timeout");
        let listed = std::process::Command::new("tasklist")
            .args(["/fi", &format!("PID eq {pid}"), "/nh"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&listed.stdout).contains("PING"),
            "the timed-out process is still running"
        );
    }

    #[test]
    fn create_new_never_overwrites() {
        let path = std::env::temp_dir().join(format!("crtest-new-{}", random_hex()));
        create_new(&path).unwrap();
        assert_eq!(create_new(&path).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        std::fs::remove_file(&path).unwrap();
    }
}
