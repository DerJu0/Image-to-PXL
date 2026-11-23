//! Bettet das App-Icon und die Versionsinfos in die Windows-Exe-Dateien ein.
//! Icon neu erzeugen: cargo run --example make_icon

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Image to PXL")
            .set("FileDescription", "Image to PXL - Bilder in Pixel-Art umwandeln");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Icon konnte nicht eingebettet werden: {e}");
        }
    }
}
