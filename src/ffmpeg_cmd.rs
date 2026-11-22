//! The capture ffmpeg's command line. Every argument here was measured on the Python app;
//! the tests pin each one to the bug it once caused.

use crate::clock::ClockLine;
use crate::config::ClipFormat;
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

impl Grabber {
    /// The -debug_ts line a capture through it reads its clock from: ddagrab is a source filter,
    /// gdigrab an input.
    pub fn clock_line(self) -> ClockLine {
        if self == Grabber::Ddagrab {
            ClockLine::Filter
        } else {
            ClockLine::Demuxer
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Codec {
    /// HEVC 10-bit 4:2:0: H.264 qp 20's quality for 18 % less, 10-bit gradients, still decoded
    /// by the GPU (4:4:4 is not, through DXVA). Measured 2026-11-22 on gameplay.
    Hevc,
    H264,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoder {
    /// HEVC where the card can (GTX 16 / RTX and later), else H.264 (GTX 9 and 10).
    Nvenc(Codec),
    /// CPU fallback when the GPU has no NVENC.
    X264,
}

impl Encoder {
    pub fn codec(self) -> Codec {
        match self {
            Encoder::Nvenc(codec) => codec,
            Encoder::X264 => Codec::H264,
        }
    }
}

/// What a clip is: its container, and the codec its video was captured in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    pub format: ClipFormat,
    pub codec: Codec,
}

/// NVENC's codec, for the capture and its probe alike. Main10 from 8-bit frames: -highbitdepth.
fn nvenc_args(codec: Codec) -> &'static [&'static str] {
    match codec {
        Codec::Hevc => &["-c:v", "hevc_nvenc", "-profile:v", "main10", "-highbitdepth", "1"],
        Codec::H264 => &["-c:v", "h264_nvenc"],
    }
}

/// The probe's encoder: the capture's, with the B-frames it uses from 120 fps. A GTX 10's HEVC
/// NVENC has none: it fails here and stays on H.264 rather than a capture that never starts.
pub fn probe_args(codec: Codec) -> Vec<String> {
    let mut a: Vec<String> = nvenc_args(codec).iter().map(|f| f.to_string()).collect();
    a.extend(["-bf", "3"].map(String::from));
    a
}

/// What one capture reads: a DXGI output, by its index on the first graphics adapter (as ddagrab
/// counts them), or the GDI desktop at a screen's rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Ddagrab { output: usize },
    Gdigrab(Monitor),
}

impl Source {
    pub fn grabber(&self) -> Grabber {
        match self {
            Source::Ddagrab { .. } => Grabber::Ddagrab,
            Source::Gdigrab(_) => Grabber::Gdigrab,
        }
    }
}

/// NVIDIA's PCI vendor id: NVENC encodes only its adapter's frames, and the app lists only its
/// outputs (monitors::dxgi_outputs).
pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

/// ddagrab as a source filter on the NVIDIA adapter, named explicitly. Read through -f lavfi it
/// ignores any device and runs on the ffmpeg process's default adapter, which a per-app GPU
/// preference can make another one than the app's (measured: lavfi keeps an internal device).
pub fn ddagrab_input(fps: u32, output: usize) -> Vec<String> {
    vec![
        "-init_hw_device".into(),
        format!("d3d11va=dda:,vendor_id={NVIDIA_VENDOR_ID:#06x}"),
        "-filter_hw_device".into(),
        "dda".into(),
        "-filter_complex".into(),
        format!("ddagrab=framerate={fps}:output_idx={output}:draw_mouse=0"),
    ]
}

/// ddagrab on the DXGI output whose rectangle is `screen`'s, else GDI on that rectangle.
/// `dxgi_outputs` are the first adapter's, in DXGI's order: Settings' order (primary first, left
/// to right) only matched it by luck, and a screen on another adapter is not in it at all.
pub fn source(grabber: Grabber, dxgi_outputs: &[Monitor], screen: Monitor) -> Source {
    match dxgi_outputs.iter().position(|o| *o == screen) {
        Some(output) if grabber == Grabber::Ddagrab => Source::Ddagrab { output },
        _ => Source::Gdigrab(screen),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Capture<'a> {
    pub fps: u32,
    pub buffer_seconds: u32,
    pub source: Source,
    pub encoder: Encoder,
    pub segment_dir: &'a Path,
}

/// Segments kept on disk: enough to cover the buffer, plus the one being written and one spare.
pub fn segment_wrap(buffer_seconds: u32) -> u32 {
    buffer_seconds.div_ceil(SEGMENT_DURATION) + 2
}

/// Arguments after the ffmpeg path.
/// The capture pipeline from real test encodes: the NVENC codec that works (if any), DXGI into it
/// if that works too, else through GDI, else x264 through GDI. Never ddagrab with x264: ddagrab
/// hands over GPU frames x264 cannot read.
pub fn choose(ddagrab_into: Option<Codec>, nvenc: Option<Codec>) -> (Encoder, Grabber) {
    match (ddagrab_into, nvenc) {
        (Some(codec), _) => (Encoder::Nvenc(codec), Grabber::Ddagrab),
        (None, Some(codec)) => (Encoder::Nvenc(codec), Grabber::Gdigrab),
        (None, None) => (Encoder::X264, Grabber::Gdigrab),
    }
}

/// DXGI first (it holds 240 fps, GDI 60), then HEVC over H.264, then any NVENC over the CPU.
fn rank(p: (Encoder, Grabber)) -> u8 {
    let codec = |e: Encoder| if e.codec() == Codec::Hevc { 1 } else { 0 };
    match p {
        (Encoder::X264, _) => 0,
        (e, Grabber::Gdigrab) => 1 + codec(e),
        (e, Grabber::Ddagrab) => 3 + codec(e),
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
    let rate = effective_fps(c.fps, c.source.grabber());
    let keyframes = (rate * SEGMENT_DURATION).to_string();
    let fps = rate.to_string();
    let wrap = segment_wrap(c.buffer_seconds).to_string();
    let mut a: Vec<String> = vec!["-y".into(), "-debug_ts".into()];
    match c.source {
        Source::Ddagrab { output } => a.extend(ddagrab_input(rate, output)),
        Source::Gdigrab(m) => a.extend([
            "-f".into(),
            "gdigrab".into(),
            "-framerate".into(),
            fps.clone(),
            "-draw_mouse".into(),
            "0".into(),
            "-offset_x".into(),
            m.x.to_string(),
            "-offset_y".into(),
            m.y.to_string(),
            "-video_size".into(),
            format!("{}x{}", m.w, m.h),
            "-i".into(),
            "desktop".into(),
        ]),
    }
    match c.encoder {
        Encoder::Nvenc(codec) => {
            a.extend(nvenc_args(codec).iter().map(|f| f.to_string()));
            a.extend(["-preset", "p1"].map(String::from));
            if rate >= B_FRAMES_MIN_FPS {
                a.extend(["-tune", "hq", "-bf", "3", "-delay", "0"].map(String::from));
            } else {
                a.extend(["-tune", "ll"].map(String::from));
            }
            a.extend(["-rc", "constqp", "-qp", "20"].map(String::from));
        }
        Encoder::X264 => a.extend(
            [
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-crf",
                "18",
                // From BGRA libx264 picks High 4:4:4, which no GPU decodes (DXVA).
                "-pix_fmt",
                "yuv420p",
            ]
            .map(String::from),
        ),
    }
    a.extend(["-g".into(), keyframes]);
    a.extend([
        "-flush_packets".into(),
        "1".into(),
        "-f".into(),
        "segment".into(),
        "-segment_time".into(),
        SEGMENT_DURATION.to_string(),
        "-segment_wrap".into(),
        wrap.clone(),
        "-reset_timestamps".into(),
        "1".into(),
        "-segment_list".into(),
        c.segment_dir.join(SEGMENT_LIST_FILE).to_string_lossy().into_owned(),
        "-segment_list_type".into(),
        "csv".into(),
        "-segment_list_size".into(),
        wrap,
        "-segment_format".into(),
        "mpegts".into(),
        c.segment_dir.join(SEGMENT_PATTERN).to_string_lossy().into_owned(),
    ]);
    a
}

/// The concat demuxer's list. Only snapshot copies go in it, never the live segments (ffmpeg
/// rewrites those under -segment_wrap). Quotes are escaped the concat way.
pub fn concat_list(files: &[std::path::PathBuf]) -> String {
    files
        .iter()
        .map(|f| {
            format!(
                "file '{}'\n",
                f.to_string_lossy().replace('\\', "/").replace('\'', r"'\''")
            )
        })
        .collect()
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Joins the snapshots without re-encoding. No -ss, not even 0: with B-frames each segment's
/// first IDR has a negative DTS, and -ss 0 dropped the whole first segment (measured).
/// `clip`: set when it writes the clip itself (no audio); None for the intermediate video the mux
/// reads next, which is never worth +faststart's rewrite.
pub fn concat_args(list: &Path, duration: f64, out: &Path, clip: Option<Clip>) -> Vec<String> {
    let mut a: Vec<String> = ["-y", "-f", "concat", "-safe", "0", "-i"].map(String::from).to_vec();
    a.extend([path(list), "-t".into(), duration.to_string()]);
    a.extend(["-c", "copy"].map(String::from));
    a.extend(clip.map_or(&[][..], clip_flags).iter().map(|f| f.to_string()));
    a.push(path(out));
    a
}

/// What the clip's container needs: an MP4 gets its index first, so players start at once
/// (+faststart is the MP4 muxer's flag, MKV has none), and HEVC in MP4 the hvc1 tag Apple players
/// require (ffmpeg writes hev1; the tag would fail on H.264).
fn clip_flags(clip: Clip) -> &'static [&'static str] {
    match (clip.format, clip.codec) {
        (ClipFormat::Mp4, Codec::Hevc) => &["-movflags", "+faststart", "-tag:v", "hvc1"],
        (ClipFormat::Mp4, Codec::H264) => &["-movflags", "+faststart"],
        (ClipFormat::Mkv, _) => &[],
    }
}

/// Game sound + mic. Both resampled to 48 kHz stereo before amix, normalize off: otherwise the
/// volume pumps or buzzes.
pub fn mix_args(loopback: &Path, mic: &Path, out: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-i".into(),
        path(loopback),
        "-i".into(),
        path(mic),
        "-filter_complex".into(),
        "[0:a]aresample=48000,aformat=channel_layouts=stereo[a0];\
         [1:a]aresample=48000,aformat=channel_layouts=stereo[a1];\
         [a0][a1]amix=inputs=2:duration=longest:normalize=0"
            .into(),
        "-ac".into(),
        "2".into(),
        "-ar".into(),
        "48000".into(),
        path(out),
    ]
}

/// FLAC is lossless for ~1 % of a clip, but many players can't read it inside an MP4. AAC 256k
/// encodes as fast as 192k; 320k took 5 to 10 s on 90 s of audio, all of it added to a save.
fn audio_codec(format: ClipFormat) -> &'static [&'static str] {
    match format {
        ClipFormat::Mp4 => &["-c:a", "aac", "-b:a", "256k"],
        ClipFormat::Mkv => &["-c:a", "flac"],
    }
}

/// The clip: the video copied, the audio per the format, cut to the shorter of the two.
pub fn mux_args(video: &Path, audio: &Path, out: &Path, clip: Clip) -> Vec<String> {
    let mut a: Vec<String> = vec!["-y".into(), "-i".into(), path(video), "-i".into(), path(audio)];
    a.extend(["-c:v", "copy", "-shortest"].map(String::from));
    a.extend(audio_codec(clip.format).iter().map(|f| f.to_string()));
    a.extend(clip_flags(clip).iter().map(|f| f.to_string()));
    a.push(path(out));
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BUFFER_OPTIONS, FPS_OPTIONS};

    const MP4: Clip = Clip {
        format: ClipFormat::Mp4,
        codec: Codec::Hevc,
    };
    const MKV: Clip = Clip {
        format: ClipFormat::Mkv,
        codec: Codec::Hevc,
    };

    /// +faststart rewrites the whole file to move its index first: worth it for the clip, wasted
    /// on the video the mux reads next (up to 4 GB written twice).
    #[test]
    fn only_the_clip_is_rewritten_for_fast_start() {
        let fast = |a: Vec<String>| a.contains(&"+faststart".to_string());
        let concat = |clip| concat_args(Path::new("l.txt"), 3.0, Path::new("out"), clip);
        let mux = |format| mux_args(Path::new("v.mp4"), Path::new("a.wav"), Path::new("out"), format);
        assert!(fast(concat(Some(MP4))));
        assert!(!fast(concat(None)), "the intermediate video");
        assert!(fast(mux(MP4)));
    }

    /// +faststart is the MP4 muxer's flag: an MKV clip gets no -movflags at all.
    #[test]
    fn an_mkv_clip_gets_no_mp4_flags() {
        let movflags = |a: Vec<String>| a.contains(&"-movflags".to_string());
        assert!(!movflags(concat_args(
            Path::new("l.txt"),
            3.0,
            Path::new("clip.mkv"),
            Some(MKV)
        )));
        assert!(!movflags(mux_args(
            Path::new("v.mp4"),
            Path::new("a.wav"),
            Path::new("clip.mkv"),
            MKV
        )));
    }

    #[test]
    fn the_concat_never_seeks_and_cuts_at_the_selected_duration() {
        let a = concat_args(Path::new("list.txt"), 15.25, Path::new("out.mp4"), Some(MP4));
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
        let a = mux_args(Path::new("v.mp4"), Path::new("a.wav"), Path::new("clip.mp4"), MP4);
        assert_eq!(
            (value(&a, "-c:v"), value(&a, "-c:a"), value(&a, "-b:a")),
            (Some("copy"), Some("aac"), Some("256k"))
        );
        assert!(a.contains(&"-shortest".to_string()) && a.contains(&"+faststart".to_string()));
    }

    /// FLAC is lossless for ~1 % of a clip's size, but many players can't read it inside an MP4.
    #[test]
    fn an_mkv_clip_keeps_lossless_flac_audio() {
        let a = mux_args(Path::new("v.mp4"), Path::new("a.wav"), Path::new("clip.mkv"), MKV);
        assert_eq!((value(&a, "-c:a"), value(&a, "-b:a")), (Some("flac"), None));
        assert!(a.contains(&"-shortest".to_string()));
    }

    #[test]
    fn the_concat_list_escapes_quotes_and_uses_forward_slashes() {
        let list = concat_list(&[r"C:\Users\O'Neil\snap_1.ts".into(), r"C:\t\snap_2.ts".into()]);
        assert_eq!(list, "file 'C:/Users/O'\\''Neil/snap_1.ts'\nfile 'C:/t/snap_2.ts'\n");
    }

    const PRIMARY: Monitor = Monitor {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    const SECOND: Monitor = Monitor {
        x: 1920,
        y: 0,
        w: 2560,
        h: 1440,
    };

    /// The capture of SECOND, DXGI's output 1 when ddagrab captures it.
    fn args(fps: u32, grabber: Grabber, encoder: Encoder) -> Vec<String> {
        capture_args(&Capture {
            fps,
            buffer_seconds: 30,
            source: source(grabber, &[PRIMARY, SECOND], SECOND),
            encoder,
            segment_dir: Path::new(r"C:\tmp\cliprec_x"),
        })
    }

    fn value<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        a.iter()
            .position(|x| x == flag)
            .and_then(|i| a.get(i + 1))
            .map(String::as_str)
    }

    const HEVC: Encoder = Encoder::Nvenc(Codec::Hevc);
    const H264: Encoder = Encoder::Nvenc(Codec::H264);

    #[test]
    fn the_pipeline_is_never_ddagrab_into_x264() {
        let (hevc, h264) = (Some(Codec::Hevc), Some(Codec::H264));
        assert_eq!(choose(hevc, hevc), (HEVC, Grabber::Ddagrab));
        assert_eq!(choose(h264, h264), (H264, Grabber::Ddagrab));
        assert_eq!(choose(None, hevc), (HEVC, Grabber::Gdigrab));
        assert_eq!(choose(None, h264), (H264, Grabber::Gdigrab));
        assert_eq!(choose(None, None), (Encoder::X264, Grabber::Gdigrab));
    }

    #[test]
    fn a_later_probe_upgrades_only_to_a_better_pipeline() {
        // Best first: DXGI beats codec (fps), HEVC beats H.264, any NVENC beats the CPU.
        let ladder = [
            (HEVC, Grabber::Ddagrab),
            (H264, Grabber::Ddagrab),
            (HEVC, Grabber::Gdigrab),
            (H264, Grabber::Gdigrab),
            (Encoder::X264, Grabber::Gdigrab),
        ];
        for (i, &high) in ladder.iter().enumerate() {
            for &low in &ladder[i + 1..] {
                assert!(better(low, high) && !better(high, low), "{high:?} over {low:?}");
            }
            assert!(!better(high, high));
        }
    }

    /// A GTX 10 card's HEVC NVENC has no B-frames, which the capture asks for from 120 fps: the
    /// probe asks for them too, so such a card fails it and stays on H.264 NVENC instead of a
    /// capture that dies at every start. Same codec arguments as the capture's.
    #[test]
    fn the_probe_tests_the_capture_s_own_encoder_with_b_frames() {
        for (codec, encoder) in [(Codec::Hevc, HEVC), (Codec::H264, H264)] {
            let probe = probe_args(codec);
            let capture = args(240, Grabber::Ddagrab, encoder);
            assert_eq!(value(&probe, "-bf"), Some("3"), "{codec:?}");
            for flag in ["-c:v", "-profile:v", "-highbitdepth"] {
                assert_eq!(value(&probe, flag), value(&capture, flag), "{codec:?} {flag}");
            }
        }
    }

    /// Apple players refuse an HEVC MP4 tagged hev1 (ffmpeg's default); the hvc1 tag would make
    /// ffmpeg fail on an H.264 stream, and MKV has no such tag.
    #[test]
    fn an_hevc_mp4_clip_is_tagged_for_apple_players() {
        let tag = |a: Vec<String>| value(&a, "-tag:v").map(String::from);
        let concat = |clip| concat_args(Path::new("l.txt"), 3.0, Path::new("out"), Some(clip));
        let mux = |clip| mux_args(Path::new("v.mp4"), Path::new("a.wav"), Path::new("out"), clip);
        for (format, codec, expected) in [
            (ClipFormat::Mp4, Codec::Hevc, Some("hvc1")),
            (ClipFormat::Mp4, Codec::H264, None),
            (ClipFormat::Mkv, Codec::Hevc, None),
            (ClipFormat::Mkv, Codec::H264, None),
        ] {
            let clip = Clip { format, codec };
            assert_eq!(tag(concat(clip)).as_deref(), expected, "{clip:?}");
            assert_eq!(tag(mux(clip)).as_deref(), expected, "{clip:?}");
        }
        assert_eq!(tag(concat_args(Path::new("l"), 3.0, Path::new("o"), None)), None);
    }

    /// GDI cannot hold 120/240: a config saved on a DXGI session is capped, keyframes and
    /// B-frames follow the capped rate.
    #[test]
    fn gdigrab_never_captures_above_60_fps() {
        let a = args(240, Grabber::Gdigrab, HEVC);
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
        for name in [
            "snap_ab12_000.ts",
            "concat_ab12.txt",
            "loopback_ab12.wav",
            "mic_ab12.wav",
            "video_ab12.mp4",
            "mixed_ab12.wav",
            "ffmpeg.pid",
            "segments.csv.tmp",
        ] {
            assert!(!is_capture_file(name), "{name} belongs to a save or the orphan sweep");
        }
    }

    #[test]
    fn the_keyframe_interval_equals_fps_times_the_segment_length() {
        // Otherwise segment boundaries miss keyframes and the concat shows artifacts.
        for fps in FPS_OPTIONS {
            for encoder in [HEVC, H264, Encoder::X264] {
                let a = args(fps, Grabber::Ddagrab, encoder);
                assert_eq!(
                    value(&a, "-g"),
                    Some((fps * SEGMENT_DURATION).to_string().as_str()),
                    "{fps} fps"
                );
                assert_eq!(value(&a, "-segment_time"), Some(SEGMENT_DURATION.to_string().as_str()));
            }
        }
    }

    #[test]
    fn packets_are_flushed_so_snapshots_are_current() {
        assert_eq!(value(&args(60, Grabber::Ddagrab, HEVC), "-flush_packets"), Some("1"));
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
        let a = args(240, Grabber::Ddagrab, HEVC).join(" ");
        assert!(!a.contains("format=yuv420p"), "{a}");
        assert!(a.contains("ddagrab=framerate=240:output_idx=1:draw_mouse=0"), "{a}");
    }

    #[test]
    fn nvenc_b_frames_start_at_120_fps_and_never_hold_output_back() {
        for (fps, encoder) in FPS_OPTIONS.into_iter().flat_map(|f| [(f, HEVC), (f, H264)]) {
            let a = args(fps, Grabber::Ddagrab, encoder);
            if fps >= B_FRAMES_MIN_FPS {
                // Without -delay 0 NVENC holds ~15 frames: +70 ms of A/V offset, measured.
                assert_eq!(
                    (value(&a, "-bf"), value(&a, "-delay"), value(&a, "-tune")),
                    (Some("3"), Some("0"), Some("hq"))
                );
            } else {
                assert_eq!((value(&a, "-bf"), value(&a, "-tune")), (None, Some("ll")), "{fps} fps");
            }
            assert_eq!(value(&a, "-qp"), Some("20"), "{encoder:?}");
        }
    }

    #[test]
    fn quality_is_the_measured_one() {
        // qp 26 was tried in v1.3: visibly worse in motion, reverted.
        assert_eq!(value(&args(60, Grabber::Ddagrab, HEVC), "-qp"), Some("20"));
        assert_eq!(value(&args(60, Grabber::Gdigrab, Encoder::X264), "-crf"), Some("18"));
    }

    /// Fed gdigrab's BGRA, libx264 wrote High 4:4:4: no GPU decodes it through DXVA and many
    /// players refuse it. Every clip without an NVIDIA GPU was one.
    #[test]
    fn the_cpu_encoder_writes_4_2_0_that_gpus_decode() {
        let a = args(60, Grabber::Gdigrab, Encoder::X264);
        assert_eq!(value(&a, "-pix_fmt"), Some("yuv420p"));
    }

    /// HEVC 10-bit 4:2:0 at qp 20: same quality as H.264 qp 20 for 18 % less, gradients in 10 bits,
    /// and still decoded by the GPU (4:4:4 is not, through DXVA). Measured 2026-11-22 on gameplay.
    #[test]
    fn nvenc_encodes_hevc_10_bit_on_both_grabbers() {
        for grabber in [Grabber::Ddagrab, Grabber::Gdigrab] {
            let a = args(240, grabber, HEVC);
            assert_eq!(
                (value(&a, "-c:v"), value(&a, "-profile:v"), value(&a, "-highbitdepth")),
                (Some("hevc_nvenc"), Some("main10"), Some("1")),
                "{grabber:?}"
            );
        }
        // The fallback tier is the H.264 line measured before HEVC.
        let a = args(240, Grabber::Ddagrab, H264);
        assert_eq!((value(&a, "-c:v"), value(&a, "-profile:v")), (Some("h264_nvenc"), None));
        assert_eq!(
            value(&args(60, Grabber::Gdigrab, Encoder::X264), "-c:v"),
            Some("libx264")
        );
    }

    #[test]
    fn gdigrab_selects_the_monitor_by_its_rectangle() {
        let a = args(60, Grabber::Gdigrab, Encoder::X264);
        assert_eq!(
            (value(&a, "-offset_x"), value(&a, "-offset_y"), value(&a, "-video_size")),
            (Some("1920"), Some("0"), Some("2560x1440"))
        );
        assert_eq!(value(&a, "-i"), Some("desktop"));
    }

    /// ddagrab read through `-f lavfi` ignores any device given to it and runs on the ffmpeg
    /// process's default adapter, which a per-app GPU preference can change. Through
    /// -filter_complex it takes the device named here: the NVIDIA adapter, which NVENC needs
    /// anyway, and whose outputs the app lists (measured: no internal device is created then).
    #[test]
    fn ddagrab_runs_on_the_nvidia_adapter_named_explicitly() {
        let a = args(240, Grabber::Ddagrab, HEVC);
        assert_eq!(value(&a, "-init_hw_device"), Some("d3d11va=dda:,vendor_id=0x10de"));
        assert_eq!(value(&a, "-filter_hw_device"), Some("dda"));
        assert_eq!(
            value(&a, "-filter_complex"),
            Some("ddagrab=framerate=240:output_idx=1:draw_mouse=0")
        );
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap();
        assert!(at("-init_hw_device") < at("-filter_hw_device") && at("-filter_hw_device") < at("-filter_complex"));
        assert!(!a.contains(&"lavfi".to_string()), "{a:?}");
        assert_eq!(Grabber::Ddagrab.clock_line(), ClockLine::Filter);
        assert_eq!(Grabber::Gdigrab.clock_line(), ClockLine::Demuxer);
    }

    /// ddagrab counts the outputs of the first graphics adapter in DXGI's own order, which is not
    /// Settings' (primary first, then left to right): the screen is found by its rectangle.
    #[test]
    fn ddagrab_captures_the_output_whose_rectangle_is_the_screen() {
        let left = Monitor {
            x: -1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let outputs = [PRIMARY, left, SECOND];
        assert_eq!(
            source(Grabber::Ddagrab, &outputs, SECOND),
            Source::Ddagrab { output: 2 }
        );
        assert_eq!(
            source(Grabber::Ddagrab, &outputs, PRIMARY),
            Source::Ddagrab { output: 0 }
        );
    }

    /// A screen on another adapter, or no DXGI list at all, is captured through GDI rather than
    /// a ddagrab that would capture another screen or fail on every restart.
    #[test]
    fn a_screen_ddagrab_cannot_reach_is_captured_through_gdi() {
        assert_eq!(source(Grabber::Ddagrab, &[PRIMARY], SECOND), Source::Gdigrab(SECOND));
        assert_eq!(source(Grabber::Ddagrab, &[], PRIMARY), Source::Gdigrab(PRIMARY));
        assert_eq!(
            source(Grabber::Gdigrab, &[PRIMARY, SECOND], SECOND),
            Source::Gdigrab(SECOND),
            "no DXGI pipeline"
        );
        assert_eq!(Source::Gdigrab(SECOND).grabber(), Grabber::Gdigrab);
        assert_eq!(Source::Ddagrab { output: 0 }.grabber(), Grabber::Ddagrab);
    }

    #[test]
    fn the_capture_prints_its_clock_and_lists_its_segments() {
        let a = args(240, Grabber::Ddagrab, HEVC);
        assert!(a.contains(&"-debug_ts".to_string()));
        assert_eq!(value(&a, "-segment_list"), Some(r"C:\tmp\cliprec_x\segments.csv"));
        assert_eq!(value(&a, "-segment_list_type"), Some("csv"));
        assert_eq!(
            value(&a, "-segment_list_size"),
            Some(segment_wrap(30).to_string().as_str())
        );
        assert_eq!(a.last().map(String::as_str), Some(r"C:\tmp\cliprec_x\seg_%03d.ts"));
    }
}
