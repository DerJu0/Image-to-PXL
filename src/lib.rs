//! Image to PXL – Bibliothek für die Pixel-Art-Umwandlung.
//! Wird vom Kommandozeilenprogramm (`pxl`) und der GUI (`pxl-gui`) genutzt.

pub mod color;
pub mod icon;
pub mod pixelate;

pub use pixelate::{pixelate, Output, Settings, DEFAULT_HEIGHT};

use image::imageops::FilterType;
use image::RgbaImage;
use std::path::Path;

/// Dateiendungen, die gelesen werden können
pub const EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];

pub fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Füllt die Pixel-Art auf exakt `tw` × `th` auf (Motiv zentriert).
/// Die Füllfarbe stammt aus der linken oberen Ecke (Hintergrund bzw. transparent).
pub fn pad_to(small: &RgbaImage, tw: u32, th: u32) -> RgbaImage {
    let (sw, sh) = small.dimensions();
    let (cw, ch) = (tw.max(sw), th.max(sh));
    let fill = *small.get_pixel(0, 0);
    let mut canvas = RgbaImage::from_pixel(cw, ch, fill);
    image::imageops::overlay(&mut canvas, small, ((cw - sw) / 2) as i64, ((ch - sh) / 2) as i64);
    canvas
}

/// Vergrößert ohne Glättung; `scale` = 0 bedeutet „ungefähr Originalgröße“.
pub fn upscale(small: &RgbaImage, scale: u32, pixel_size: f32) -> RgbaImage {
    let s = if scale > 0 { scale } else { (pixel_size.round() as u32).max(1) };
    let (w, h) = small.dimensions();
    image::imageops::resize(small, w * s, h * s, FilterType::Nearest)
}
