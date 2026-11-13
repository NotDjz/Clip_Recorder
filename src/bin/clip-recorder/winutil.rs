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
    fn create_new_never_overwrites() {
        let path = std::env::temp_dir().join(format!("crtest-new-{}", random_hex()));
        create_new(&path).unwrap();
        assert_eq!(create_new(&path).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        std::fs::remove_file(&path).unwrap();
    }
}
