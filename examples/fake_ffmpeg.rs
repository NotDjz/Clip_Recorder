//! A stand-in for ffmpeg in the save tests (built by `cargo test --examples`, never shipped).
//! It writes its arguments into its output file (the last argument) and exits 0, so a test can
//! read which inputs a step was given. Copied under another name it misbehaves on purpose:
//! `ffmpeg-failmix` fails the mix, `ffmpeg-failmux` writes a partial file then fails the mux,
//! `ffmpeg-empty` writes an empty output.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let step = if args.iter().any(|a| a == "concat") {
        "concat"
    } else if args.iter().any(|a| a.contains("amix")) {
        "mix"
    } else {
        "mux"
    };
    // No arguments: `cargo test --examples` running it as a test. Nothing to do.
    let Some(out) = args.last() else { return };
    if name.ends_with("failmix") && step == "mix" {
        std::process::exit(1); // nothing written
    }
    let content = if name.ends_with("empty") {
        String::new()
    } else {
        format!("{step}: {}", args.join(" "))
    };
    std::fs::write(out, content).unwrap();
    // failmux: a partial file, then a failure.
    std::process::exit(if name.ends_with("failmux") && step == "mux" {
        1
    } else {
        0
    });
}
