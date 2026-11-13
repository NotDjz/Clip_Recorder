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

/// A UTC timestamp in RFC 3339, millisecond precision: `2026-11-13T02:18:21.123Z`. Local time
/// in a log shifts with daylight saving and cannot be compared across machines.
pub fn rfc3339(unix_ms: u64) -> String {
    let (secs, ms) = (unix_ms / 1000, unix_ms % 1000);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm), valid from 1970 on.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_794_536_301_123), "2026-11-13T02:18:21.123Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z", "leap day");
        assert_eq!(
            rfc3339(4_107_542_399_999),
            "2100-02-28T23:59:59.999Z",
            "2100 is not a leap year"
        );
        assert_eq!(rfc3339(4_107_542_400_000), "2100-03-01T00:00:00.000Z");
        assert_eq!(rfc3339(7_258_118_400_000), "2200-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(13_569_465_599_000), "2399-12-31T23:59:59.000Z");
        assert_eq!(
            rfc3339(13_574_606_400_000),
            "2400-02-29T12:00:00.000Z",
            "2400 is a leap year"
        );
    }

    #[test]
    fn the_log_cap_is_two_mebibytes() {
        assert_eq!(LOG_MAX_BYTES, 2 << 20);
    }

    /// Right after a cut the file holds the newest half of the cap, from a whole line.
    #[test]
    fn a_cut_keeps_the_newest_half() {
        let path = std::env::temp_dir().join(format!("crtest_logcut_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cap = 4096;
        let mut i = 0;
        while !std::fs::read_to_string(&path)
            .unwrap_or_default()
            .starts_with("[log truncated]")
        {
            append_capped(&path, &format!("line {i:05} padding padding padding"), cap).unwrap();
            i += 1;
        }
        let len = std::fs::metadata(&path).unwrap().len();
        std::fs::remove_file(&path).unwrap();
        assert!(
            len > cap / 2 - 64 && len <= cap / 2 + 16,
            "{len} bytes after a cut at {cap}"
        );
    }

    #[test]
    fn the_log_is_capped_keeps_the_newest_lines_and_cuts_at_a_line() {
        let path = std::env::temp_dir().join(format!("crtest_log_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cap = 64 * 1024;
        for i in 0..4000 {
            append_capped(
                &path,
                &format!("[2026-11-13 01:00:00] line {i:05} padding padding"),
                cap,
            )
            .unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(text.len() as u64 <= cap, "{} bytes over a {cap} cap", text.len());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[log truncated]");
        assert!(
            lines[1].starts_with("[20"),
            "the cut left a partial line: {:?}",
            lines[1]
        );
        assert!(lines.last().unwrap().contains("line 03999"), "the newest line is gone");
    }
}
