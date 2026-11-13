//! Small Win32 helpers shared by the app's modules.

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// 128 random bits from the system CSPRNG, as 32 hex digits: names for temporary files nobody
/// can predict (CWE-378), combined with create_new so none is ever overwritten.
pub fn random_hex() -> String {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let mut bytes = [0u8; 16];
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 16, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    assert!(status >= 0, "BCryptGenRandom failed: {status:#x}");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new file, never an existing one (AlreadyExists otherwise).
pub fn create_new(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_names_are_128_bits_of_hex_and_never_repeat() {
        let (a, b) = (random_hex(), random_hex());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn create_new_never_overwrites() {
        let path = std::env::temp_dir().join(format!("crtest-new-{}", random_hex()));
        create_new(&path).unwrap();
        assert_eq!(create_new(&path).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        std::fs::remove_file(&path).unwrap();
    }
}
