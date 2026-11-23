//! Erzeugt assets/icon.ico (16, 32, 48, 64, 128, 256 px) und assets/icon.png.
//! Aufruf: cargo run --example make_icon

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::ExtendedColorType;
use image_to_pxl::icon::icon_image;

fn main() -> anyhow::Result<()> {
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
