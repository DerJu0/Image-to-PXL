//! App-Icon als 16×16-Pixel-Art: ein Landschaftsbild (Himmel, Sonne,
//! verschneiter Berg) im Rahmen – passend zum Zweck der App.
//! Wird für das Windows-Exe-Icon (assets/icon.ico) und das Fenster-Icon genutzt.

use image::{Rgba, RgbaImage};

const ART: [&str; 16] = [
    "..KKKKKKKKKKKK..",
    ".KBBBBBBBBBBBBK.",
    "KBBBBBBBBBBYYBBK",
    "KBBccBBBBBYYYYBK",
    "KBccccBBBBYYYYBK",
    "KBBBBBBBBBBYYBBK",
    "KbbbbbbbbbbbbbbK",
    "KbbbbbWbbbbbbbbK",
    "KbbbbWWWbbbbbbbK",
    "KbbbWGGGWbbbWbbK",
    "KbbGGGgGGGbWWWbK",
    "KbGGGggGGGGGGGGK",
    "KGGGgggGGDDGGGDK",
    "KDGGGgGDDDDDGDDK",
    ".KDDDDDDDDDDDDK.",
    "..KKKKKKKKKKKK..",
];

fn color(c: char) -> Rgba<u8> {
    match c {
        'K' => Rgba([0x1a, 0x1c, 0x2c, 255]), // Kontur
        'B' => Rgba([0x3b, 0x5d, 0xc9, 255]), // Himmel oben
        'b' => Rgba([0x41, 0xa6, 0xf6, 255]), // Himmel unten
        'c' => Rgba([0xc7, 0xdc, 0xf5, 255]), // Wolke
        'Y' => Rgba([0xff, 0xcd, 0x75, 255]), // Sonne
        'W' => Rgba([0xf4, 0xf4, 0xf4, 255]), // Schnee
        'G' => Rgba([0x38, 0xb7, 0x64, 255]), // Berg
        'g' => Rgba([0xa7, 0xf0, 0x70, 255]), // Berg hell
        'D' => Rgba([0x25, 0x71, 0x79, 255]), // Berg dunkel
        _ => Rgba([0, 0, 0, 0]),
    }
}

/// Icon in `16 * scale` Pixeln Kantenlänge (pixelgenau, ohne Glättung)
pub fn icon_image(scale: u32) -> RgbaImage {
    let s = scale.max(1);
    let mut img = RgbaImage::new(16 * s, 16 * s);
    for (y, row) in ART.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let c = color(ch);
            for dy in 0..s {
                for dx in 0..s {
                    img.put_pixel(x as u32 * s + dx, y as u32 * s + dy, c);
                }
            }
        }
    }
    img
}
