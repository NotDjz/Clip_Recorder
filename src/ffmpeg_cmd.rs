//! The capture ffmpeg's command line. Every argument here was measured on the Python app;
//! the tests pin each one to the bug it once caused.

use std::path::Path;

/// Each segment's length. The segment ffmpeg is still writing is unusable, so this is exactly
/// how much of the moment before the hotkey a clip can miss.
pub const SEGMENT_DURATION: u32 = 1;
/// From this fps up, 3 B-frames (-20 % size at 240 fps, -26 % at 120, same VMAF). Their
/// 3-frame reorder lag was never measured below it.
pub const B_FRAMES_MIN_FPS: u32 = 120;
pub const SEGMENT_LIST_FILE: &str = "segments.csv";
pub const SEGMENT_PATTERN: &str = "seg_%03d.ts";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grabber {
    /// DXGI desktop duplication, by output index.
    Ddagrab,
    /// GDI, by screen rectangle. The fallback when ffmpeg lacks ddagrab.
    Gdigrab,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoder {
    Nvenc,
    /// CPU fallback when the GPU has no NVENC.
    X264,
}

#[derive(Debug, Clone, Copy)]
pub struct Capture<'a> {
    pub fps: u32,
    pub buffer_seconds: u32,
    pub monitor_index: usize,
    pub monitor: Monitor,
    pub grabber: Grabber,
    pub encoder: Encoder,
    pub segment_dir: &'a Path,
}

/// Segments kept on disk: enough to cover the buffer, plus the one being written and one spare.
pub fn segment_wrap(buffer_seconds: u32) -> u32 {
    buffer_seconds.div_ceil(SEGMENT_DURATION) + 2
}

/// Arguments after the ffmpeg path.
/// The capture pipeline from two real test encodes: DXGI into NVENC, else NVENC through GDI, else
/// x264 through GDI. Never ddagrab with x264: ddagrab hands over GPU frames x264 cannot read.
pub fn choose(ddagrab_into_nvenc: bool, nvenc: bool) -> (Encoder, Grabber) {
    match (ddagrab_into_nvenc, nvenc) {
        (true, _) => (Encoder::Nvenc, Grabber::Ddagrab),
        (false, true) => (Encoder::Nvenc, Grabber::Gdigrab),
        (false, false) => (Encoder::X264, Grabber::Gdigrab),
    }
}

fn rank(p: (Encoder, Grabber)) -> u8 {
    match p {
        (Encoder::Nvenc, Grabber::Ddagrab) => 2,
        (Encoder::Nvenc, Grabber::Gdigrab) => 1,
        _ => 0,
    }
}

/// Whether a later probe found a better pipeline than the launch one (a driver not ready at
/// sign-in must not leave the whole session on the CPU).
pub fn better(old: (Encoder, Grabber), new: (Encoder, Grabber)) -> bool {
    rank(new) > rank(old)
}

/// GDI cannot keep up above 60 fps: a 120/240 saved while DXGI worked is capped.
pub fn effective_fps(fps: u32, grabber: Grabber) -> u32 {
    if grabber == Grabber::Gdigrab { fps.min(60) } else { fps }
}

/// The live capture's own files, which a restart wipes. Never a running save's snapshots, list,
/// WAVs, video or mix, and never the PID file the orphan sweep needs.
pub fn is_capture_file(name: &str) -> bool {
    (name.starts_with("seg_") && name.ends_with(".ts")) || name == SEGMENT_LIST_FILE
}

pub fn capture_args(c: &Capture) -> Vec<String> {
    let rate = effective_fps(c.fps, c.grabber);
    let keyframes = (rate * SEGMENT_DURATION).to_string();
    let fps = rate.to_string();
    let wrap = segment_wrap(c.buffer_seconds).to_string();
    let m = c.monitor;
    let mut a: Vec<String> = vec!["-y".into(), "-debug_ts".into()];
    match c.grabber {
        Grabber::Ddagrab => a.extend([
            "-f".into(), "lavfi".into(),
            "-i".into(), format!("ddagrab=framerate={fps}:output_idx={}:draw_mouse=0", c.monitor_index),
        ]),
        Grabber::Gdigrab => a.extend([
            "-f".into(), "gdigrab".into(), "-framerate".into(), fps.clone(), "-draw_mouse".into(), "0".into(),
            "-offset_x".into(), m.x.to_string(), "-offset_y".into(), m.y.to_string(),
            "-video_size".into(), format!("{}x{}", m.w, m.h), "-i".into(), "desktop".into(),
        ]),
    }
    match c.encoder {
        Encoder::Nvenc => {
            a.extend(["-c:v", "h264_nvenc", "-preset", "p1"].map(String::from));
            if rate >= B_FRAMES_MIN_FPS {
                a.extend(["-tune", "hq", "-bf", "3", "-delay", "0"].map(String::from));
            } else {
                a.extend(["-tune", "ll"].map(String::from));
            }
            a.extend(["-rc", "constqp", "-qp", "20"].map(String::from));
        }
        Encoder::X264 => a.extend(["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-crf", "18"].map(String::from)),
    }
    a.extend(["-g".into(), keyframes]);
    a.extend([
        "-flush_packets".into(), "1".into(),
        "-f".into(), "segment".into(),
        "-segment_time".into(), SEGMENT_DURATION.to_string(),
        "-segment_wrap".into(), wrap.clone(),
        "-reset_timestamps".into(), "1".into(),
        "-segment_list".into(), c.segment_dir.join(SEGMENT_LIST_FILE).to_string_lossy().into_owned(),
        "-segment_list_type".into(), "csv".into(),
        "-segment_list_size".into(), wrap,
        "-segment_format".into(), "mpegts".into(),
        c.segment_dir.join(SEGMENT_PATTERN).to_string_lossy().into_owned(),
    ]);
    a
}

/// The concat demuxer's list. Only snapshot copies go in it, never the live segments (ffmpeg
/// rewrites those under -segment_wrap). Quotes are escaped the concat way.
pub fn concat_list(files: &[std::path::PathBuf]) -> String {
    files
        .iter()
        .map(|f| format!("file '{}'\n", f.to_string_lossy().replace('\\', "/").replace('\'', r"'\''")))
        .collect()
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Joins the snapshots without re-encoding. No -ss, not even 0: with B-frames each segment's
/// first IDR has a negative DTS, and -ss 0 dropped the whole first segment (measured).
pub fn concat_args(list: &Path, duration: f64, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-y", "-f", "concat", "-safe", "0", "-i"].map(String::from).to_vec();
    a.extend([path(list), "-t".into(), duration.to_string()]);
    a.extend(["-c", "copy", "-movflags", "+faststart"].map(String::from));
    a.push(path(out));
    a
}

/// Game sound + mic. Both resampled to 48 kHz stereo before amix, normalize off: otherwise the
/// volume pumps or buzzes.
pub fn mix_args(loopback: &Path, mic: &Path, out: &Path) -> Vec<String> {
    vec![
        "-y".into(), "-i".into(), path(loopback), "-i".into(), path(mic),
        "-filter_complex".into(),
        "[0:a]aresample=48000,aformat=channel_layouts=stereo[a0];\
         [1:a]aresample=48000,aformat=channel_layouts=stereo[a1];\
         [a0][a1]amix=inputs=2:duration=longest:normalize=0".into(),
        "-ac".into(), "2".into(), "-ar".into(), "48000".into(), path(out),
    ]
}

/// The clip: the video copied, the audio in AAC, cut to the shorter of the two.
pub fn mux_args(video: &Path, audio: &Path, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec!["-y".into(), "-i".into(), path(video), "-i".into(), path(audio)];
    a.extend(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart"].map(String::from));
    a.push(path(out));
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BUFFER_OPTIONS, FPS_OPTIONS};

    #[test]
    fn the_concat_never_seeks_and_cuts_at_the_selected_duration() {
        let a = concat_args(Path::new("list.txt"), 15.25, Path::new("out.mp4"));
        assert!(!a.contains(&"-ss".to_string()), "{a:?}");
        assert_eq!(value(&a, "-t"), Some("15.25"));
        assert_eq!(value(&a, "-c"), Some("copy"));
        assert_eq!(a.last().map(String::as_str), Some("out.mp4"));
    }

    #[test]
    fn the_mix_resamples_before_amix_with_normalize_off() {
        let a = mix_args(Path::new("lb.wav"), Path::new("mic.wav"), Path::new("mixed.wav")).join(" ");
        assert!(a.contains("aresample=48000") && a.contains("normalize=0"), "{a}");
        assert!(a.find("aresample").unwrap() < a.find("amix").unwrap());
    }

    #[test]
    fn the_mux_copies_the_video_and_stops_at_the_shorter_stream() {
        let a = mux_args(Path::new("v.mp4"), Path::new("a.wav"), Path::new("clip.mp4"));
        assert_eq!((value(&a, "-c:v"), value(&a, "-c:a"), value(&a, "-b:a")), (Some("copy"), Some("aac"), Some("192k")));
        assert!(a.contains(&"-shortest".to_string()) && a.contains(&"+faststart".to_string()));
    }

    #[test]
    fn the_concat_list_escapes_quotes_and_uses_forward_slashes() {
        let list = concat_list(&[r"C:\Users\O'Neil\snap_1.ts".into(), r"C:\t\snap_2.ts".into()]);
        assert_eq!(list, "file 'C:/Users/O'\\''Neil/snap_1.ts'\nfile 'C:/t/snap_2.ts'\n");
    }

    const PRIMARY: Monitor = Monitor { x: 0, y: 0, w: 1920, h: 1080 };
    const SECOND: Monitor = Monitor { x: 1920, y: 0, w: 2560, h: 1440 };

    fn args(fps: u32, grabber: Grabber, encoder: Encoder) -> Vec<String> {
        capture_args(&Capture {
            fps,
            buffer_seconds: 30,
            monitor_index: 1,
            monitor: SECOND,
            grabber,
            encoder,
            segment_dir: Path::new(r"C:\tmp\cliprec_x"),
        })
    }

    fn value<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1)).map(String::as_str)
    }

    #[test]
    fn the_pipeline_is_never_ddagrab_into_x264() {
        assert_eq!(choose(true, true), (Encoder::Nvenc, Grabber::Ddagrab));
        assert_eq!(choose(false, true), (Encoder::Nvenc, Grabber::Gdigrab));
        assert_eq!(choose(false, false), (Encoder::X264, Grabber::Gdigrab));
        for (a, b) in [(true, false), (false, false), (false, true), (true, true)] {
            assert!(choose(a, b) != (Encoder::X264, Grabber::Ddagrab));
        }
    }

    #[test]
    fn a_later_probe_upgrades_only_to_a_better_pipeline() {
        let (best, gdi, cpu) = ((Encoder::Nvenc, Grabber::Ddagrab), (Encoder::Nvenc, Grabber::Gdigrab), (Encoder::X264, Grabber::Gdigrab));
        assert!(better(cpu, gdi) && better(cpu, best) && better(gdi, best));
        assert!(!better(best, best) && !better(best, gdi) && !better(gdi, cpu) && !better(cpu, cpu));
    }

    /// GDI cannot hold 120/240: a config saved on a DXGI session is capped, keyframes and
    /// B-frames follow the capped rate.
    #[test]
    fn gdigrab_never_captures_above_60_fps() {
        let a = args(240, Grabber::Gdigrab, Encoder::Nvenc);
        assert_eq!(value(&a, "-framerate"), Some("60"));
        assert_eq!(value(&a, "-g"), Some("60"));
        assert!(!a.contains(&"-bf".to_string()), "B-frames are for 120 fps and up");
        assert_eq!(effective_fps(240, Grabber::Ddagrab), 240);
        assert_eq!(effective_fps(30, Grabber::Gdigrab), 30);
    }

    #[test]
    fn a_restart_wipes_only_the_live_captures_files() {
        for name in ["seg_000.ts", "seg_121.ts", SEGMENT_LIST_FILE] {
            assert!(is_capture_file(name), "{name}");
        }
        for name in ["snap_ab12_000.ts", "concat_ab12.txt", "loopback_ab12.wav", "mic_ab12.wav", "video_ab12.mp4", "mixed_ab12.wav", "ffmpeg.pid", "segments.csv.tmp"] {
            assert!(!is_capture_file(name), "{name} belongs to a save or the orphan sweep");
        }
    }

    #[test]
    fn the_keyframe_interval_equals_fps_times_the_segment_length() {
        // Otherwise segment boundaries miss keyframes and the concat shows artifacts.
        for fps in FPS_OPTIONS {
            for encoder in [Encoder::Nvenc, Encoder::X264] {
                let a = args(fps, Grabber::Ddagrab, encoder);
                assert_eq!(value(&a, "-g"), Some((fps * SEGMENT_DURATION).to_string().as_str()), "{fps} fps");
                assert_eq!(value(&a, "-segment_time"), Some(SEGMENT_DURATION.to_string().as_str()));
            }
        }
    }

    #[test]
    fn packets_are_flushed_so_snapshots_are_current() {
        assert_eq!(value(&args(60, Grabber::Ddagrab, Encoder::Nvenc), "-flush_packets"), Some("1"));
    }

    #[test]
    fn the_segment_wrap_covers_every_buffer() {
        for b in BUFFER_OPTIONS {
            assert!(segment_wrap(b) * SEGMENT_DURATION >= b + 2 * SEGMENT_DURATION, "{b} s");
        }
        assert_eq!(segment_wrap(120), 122);
    }

    #[test]
    fn ddagrab_with_nvenc_has_no_pixel_format_filter_and_picks_the_output() {
        // "-vf format=yuv420p" with ddagrab + NVENC fails with "Invalid argument".
        let a = args(240, Grabber::Ddagrab, Encoder::Nvenc).join(" ");
        assert!(!a.contains("format=yuv420p"), "{a}");
        assert!(a.contains("ddagrab=framerate=240:output_idx=1:draw_mouse=0"), "{a}");
    }

    #[test]
    fn nvenc_b_frames_start_at_120_fps_and_never_hold_output_back() {
        for fps in FPS_OPTIONS {
            let a = args(fps, Grabber::Ddagrab, Encoder::Nvenc);
            if fps >= B_FRAMES_MIN_FPS {
                // Without -delay 0 NVENC holds ~15 frames: +70 ms of A/V offset, measured.
                assert_eq!((value(&a, "-bf"), value(&a, "-delay"), value(&a, "-tune")), (Some("3"), Some("0"), Some("hq")));
            } else {
                assert_eq!((value(&a, "-bf"), value(&a, "-tune")), (None, Some("ll")), "{fps} fps");
            }
        }
    }

    #[test]
    fn quality_is_the_measured_one() {
        // qp 26 was tried in v1.3: visibly worse in motion, reverted.
        assert_eq!(value(&args(60, Grabber::Ddagrab, Encoder::Nvenc), "-qp"), Some("20"));
        assert_eq!(value(&args(60, Grabber::Gdigrab, Encoder::X264), "-crf"), Some("18"));
    }

    #[test]
    fn gdigrab_selects_the_monitor_by_its_rectangle() {
        let a = args(60, Grabber::Gdigrab, Encoder::X264);
        assert_eq!((value(&a, "-offset_x"), value(&a, "-offset_y"), value(&a, "-video_size")), (Some("1920"), Some("0"), Some("2560x1440")));
        assert_eq!(value(&a, "-i"), Some("desktop"));
        let _ = PRIMARY;
    }

    #[test]
    fn the_capture_prints_its_clock_and_lists_its_segments() {
        let a = args(240, Grabber::Ddagrab, Encoder::Nvenc);
        assert!(a.contains(&"-debug_ts".to_string()));
        assert_eq!(value(&a, "-segment_list"), Some(r"C:\tmp\cliprec_x\segments.csv"));
        assert_eq!(value(&a, "-segment_list_type"), Some("csv"));
        assert_eq!(value(&a, "-segment_list_size"), Some(segment_wrap(30).to_string().as_str()));
        assert_eq!(a.last().map(String::as_str), Some(r"C:\tmp\cliprec_x\seg_%03d.ts"));
    }
}
