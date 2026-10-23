// Embeds assets/icon.ico in the Windows executables. Explorer, the taskbar and the installer's
// Start Menu / desktop shortcuts all take their icon from the .exe, so without this they show a
// generic one (the window icon set in main.rs only covers the running window).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("embed assets/icon.ico");
    }
}
