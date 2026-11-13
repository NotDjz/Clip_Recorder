// Embeds the icon and file properties in clip-recorder.exe
// (Explorer, taskbar, Start Menu shortcut, Task Manager, the tray and window icons).
fn main() {
    println!("cargo:rerun-if-changed=assets/clip-recorder.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/clip-recorder.ico")
            .set("ProductName", "Clip Recorder")
            .set("FileDescription", "Clip Recorder")
            .set("CompanyName", "NotDjz")
            .compile()
            .expect("embed Windows resources (needs a resource compiler: rc.exe or windres)");
    }
}
