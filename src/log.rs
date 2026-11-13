//! clip_recorder.log: the diagnostic trail real desync reports were solved from. Capped, so a
//! long-running install never fills the disk, and cut at a line so it stays readable.

use std::path::Path;

pub const LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
const TRUNCATED: &str = "[log truncated]\n";

/// Appends `line` and a newline. Past `cap` bytes, keeps the newest half, starting at a whole line.
pub fn append_capped(path: &Path, line: &str, cap: u64) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    if file.metadata()?.len() <= cap {
        return Ok(());
    }
    drop(file);
    let text = std::fs::read(path)?;
    let tail = &text[text.len() - (cap / 2) as usize..];
    let whole = tail.iter().position(|&b| b == b'\n').map_or(tail, |i| &tail[i + 1..]);
    let mut kept = TRUNCATED.as_bytes().to_vec();
    kept.extend_from_slice(whole);
    std::fs::write(path, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_capped_keeps_the_newest_lines_and_cuts_at_a_line() {
        let path = std::env::temp_dir().join(format!("crtest_log_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cap = 64 * 1024;
        for i in 0..4000 {
            append_capped(&path, &format!("[2026-11-13 01:00:00] line {i:05} padding padding"), cap).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(text.len() as u64 <= cap, "{} bytes over a {cap} cap", text.len());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[log truncated]");
        assert!(lines[1].starts_with("[20"), "the cut left a partial line: {:?}", lines[1]);
        assert!(lines.last().unwrap().contains("line 03999"), "the newest line is gone");
    }
}
