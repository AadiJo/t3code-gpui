//! Embeds the app icon in the Windows executables (Explorer, taskbar and the
//! window icon GPUI loads as resource 1). Other platforms do nothing.

use std::path::PathBuf;

fn main() {
    let icon = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../../assets/prod/t3-black-windows.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // An absolute path, so rc.exe finds the icon whatever directory it runs in.
    let icon = icon.canonicalize().expect("the Windows icon exists");
    let rc = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("app-icon.rc");
    let escaped = icon.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{escaped}\"\n")).expect("write the icon resource script");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("compile the icon resource");
}
