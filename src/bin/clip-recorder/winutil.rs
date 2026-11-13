//! Small Win32 helpers shared by the app's modules.

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
