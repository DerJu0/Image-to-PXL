//! Erzeugt die App-Icons aus src/icon.rs.
//!
//!   cargo run --example make_icon
//!       -> assets/icon.ico (16–256 px, Windows) und assets/icon.png
//!
//!   cargo run --example make_icon -- --iconset <Ordner>.iconset
//!       -> PNG-Satz für macOS; danach `iconutil -c icns <Ordner>.iconset` (nur auf dem Mac)

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::ExtendedColorType;
use image_to_pxl::icon::icon_image;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--iconset") {
        let dir = PathBuf::from(args.get(i + 1).map(String::as_str).unwrap_or("AppIcon.iconset"));
        return write_iconset(&dir);
    }

    std::fs::create_dir_all("assets")?;
    let frames = [1u32, 2, 3, 4, 8, 16]
        .iter()
        .map(|&s| {
            let img = icon_image(s);
            IcoFrame::as_png(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()?;
    IcoEncoder::new(std::fs::File::create("assets/icon.ico")?).encode_images(&frames)?;
    icon_image(16).save("assets/icon.png")?;
    println!("assets/icon.ico und assets/icon.png geschrieben");
    Ok(())
}

/// macOS-Iconset: icon_<n>x<n>.png und icon_<n>x<n>@2x.png für n = 16 … 512
fn write_iconset(dir: &PathBuf) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for n in [16u32, 32, 128, 256, 512] {
        icon_image(n / 16).save(dir.join(format!("icon_{n}x{n}.png")))?;
        icon_image(n * 2 / 16).save(dir.join(format!("icon_{n}x{n}@2x.png")))?;
    }
    println!("{} geschrieben", dir.display());
    Ok(())
}
