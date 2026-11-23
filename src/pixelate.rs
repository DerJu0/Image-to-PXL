//! Kern-Algorithmus der Pixel-Art-Umwandlung.
//!
//! Ablauf:
//! 1. Hintergrund erkennen (Flood-Fill von den Bildecken, wenn diese einfarbig sind)
//! 2. Optional auf den Inhalt zuschneiden
//! 3. Bild in Zellen (= spätere Pixel) einteilen; pro Zelle eine *dominante*
//!    Farbe bestimmen statt eines matschigen Mittelwerts. Dunkle Linien (Lineart)
//!    werden separat erkannt und bevorzugt, damit Konturen erhalten bleiben.
//! 4. Begrenzte Palette per k-Means in Oklab bilden und alle Zellen darauf abbilden
//! 5. Aufräumen: isolierte Einzelpixel entfernen
//! 6. Optional dunkle Außenkontur (Selective Outline) um die Figur ziehen

use crate::color::{lab_to_rgb, rgb_to_lab, Lab};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Kantenlänge eines Pixel-Blocks im Originalbild (0 = aus `width`/`height` berechnen)
    pub pixel_size: u32,
    /// Gewünschte Breite der Pixel-Art in Pixeln
    pub width: Option<u32>,
    /// Gewünschte Höhe der Pixel-Art in Pixeln
    pub height: Option<u32>,
    pub colors: usize,
    pub detect_background: bool,
    pub transparent: bool,
    pub crop: bool,
    pub outline: bool,
    pub cleanup: bool,
    pub saturation: f32,
    pub contrast: f32,
    /// Wie stark ein Pixel dunkler als seine Umgebung sein muss, um als Linie zu gelten (Oklab L)
    pub line_delta: f32,
    /// Anteil Linien-Pixel in einer Zelle, ab dem die Zelle zur Linie wird
    pub line_threshold: f32,
}

pub struct Output {
    /// Pixel-Art in Originalraster-Größe (1 Pixel = 1 Zelle)
    pub small: RgbaImage,
    /// Blockgröße in Originalpixeln (kann bei exakter Zielauflösung gebrochen sein)
    pub pixel_size: f32,
    pub palette_size: usize,
}

#[derive(Clone, Copy, Default)]
struct Cell {
    fg: bool,
    line: bool,
    lab: Lab,
    /// Farbe der Zelle ohne Linienanteil (Ersatz, falls die Linie verworfen wird)
    body: Lab,
}

pub fn pixelate(img: &RgbaImage, s: &Settings) -> Output {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();

    // ---------- 1. Hintergrund ----------
    let (bg_mask, bg_color) = if s.detect_background {
        detect_background(img)
    } else {
        (None, None)
    };
    let is_bg = |x: u32, y: u32| -> bool {
        let i = (y * w + x) as usize;
        raw[i * 4 + 3] < 128 || bg_mask.as_ref().is_some_and(|m| m[i])
    };

    // ---------- 2. Zuschneiden ----------
    let (mut x0, mut y0, mut x1, mut y1) = (0u32, 0u32, w, h);
    if s.crop {
        if let Some((bx0, by0, bx1, by1)) = content_bbox(w, h, &is_bg) {
            (x0, y0, x1, y1) = (bx0, by0, bx1, by1);
        }
    }

    if s.crop {
        // zwei Pixel Rand rundherum, damit die Außenkontur Platz hat
        let (_, ch0) = grid_size(x1 - x0, y1 - y0, s);
        let margin = ((y1 - y0) as f32 / ch0 as f32 * 2.0).ceil() as u32;
        x0 = x0.saturating_sub(margin);
        y0 = y0.saturating_sub(margin);
        x1 = (x1 + margin).min(w);
        y1 = (y1 + margin).min(h);
    }
    let (cw, ch) = grid_size(x1 - x0, y1 - y0, s);
    // Gebrochene Blockgrößen, damit das Raster exakt die Zielauflösung trifft
    let bx = (x1 - x0) as f32 / cw as f32;
    let by = (y1 - y0) as f32 / ch as f32;
    let block = bx.max(by);

    let cell_rect = |cx: u32, cy: u32| {
        let sx = x0 + (cx as f32 * bx) as u32;
        let sy = y0 + (cy as f32 * by) as u32;
        let ex = (x0 + ((cx + 1) as f32 * bx) as u32).clamp(sx + 1, x1);
        let ey = (y0 + ((cy + 1) as f32 * by) as u32).clamp(sy + 1, y1);
        (sx, sy, ex, ey)
    };

    // ---------- 3a. Mittlere Helligkeit pro Zelle (für Linienerkennung) ----------
    let mean_l: Vec<Option<f32>> = (0..ch * cw)
        .into_par_iter()
        .map(|idx| {
            let (cx, cy) = (idx % cw, idx / cw);
            let (sx, sy, ex, ey) = cell_rect(cx, cy);
            let (mut sum, mut n, mut total) = (0f32, 0u32, 0u32);
            for y in sy..ey {
                for x in sx..ex {
                    total += 1;
                    if is_bg(x, y) {
                        continue;
                    }
                    let i = ((y * w + x) * 4) as usize;
                    sum += rgb_to_lab(raw[i], raw[i + 1], raw[i + 2]).l;
                    n += 1;
                }
            }
            // Zelle gilt als Hintergrund, wenn sie überwiegend Hintergrund ist
            if n * 2 < total { None } else { Some(sum / n as f32) }
        })
        .collect();

    // ---------- 3b. Repräsentative Farbe pro Zelle ----------
    let cells: Vec<Cell> = (0..ch * cw)
        .into_par_iter()
        .map(|idx| {
            let (cx, cy) = (idx % cw, idx / cw);
            if mean_l[idx as usize].is_none() {
                return Cell::default();
            }
            // lokale Umgebungshelligkeit = Mittel der 3x3 Nachbarzellen
            let (mut sum, mut n) = (0f32, 0f32);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= cw as i32 || ny >= ch as i32 {
                        continue;
                    }
                    if let Some(v) = mean_l[(ny as u32 * cw + nx as u32) as usize] {
                        sum += v;
                        n += 1.0;
                    }
                }
            }
            let local = sum / n;

            let (sx, sy, ex, ey) = cell_rect(cx, cy);
            let mut body: Vec<Lab> = Vec::with_capacity(((ex - sx) * (ey - sy)) as usize);
            let mut line_sum = Lab::default();
            let mut line_n = 0u32;
            for y in sy..ey {
                for x in sx..ex {
                    if is_bg(x, y) {
                        continue;
                    }
                    let i = ((y * w + x) * 4) as usize;
                    let c = rgb_to_lab(raw[i], raw[i + 1], raw[i + 2]);
                    if c.l < local - s.line_delta && c.l < 0.55 {
                        line_sum.add(&c);
                        line_n += 1;
                    } else {
                        body.push(c);
                    }
                }
            }
            let fg_n = line_n as usize + body.len();
            let line_frac = line_n as f32 / fg_n.max(1) as f32;
            let line_lab = line_sum.scale(1.0 / line_n.max(1) as f32);
            let body_lab = if body.is_empty() { line_lab } else { dominant(&body) };
            if line_n > 0 && (line_frac >= s.line_threshold || body.is_empty()) {
                Cell { fg: true, line: true, lab: line_lab, body: body_lab }
            } else {
                Cell { fg: true, line: false, lab: body_lab, body: body_lab }
            }
        })
        .collect();

    // Einzelne Linienzellen ohne verbundene Linie sind meist Schattierungsrauschen
    // (Stofffalten etc.) -> durch die Flächenfarbe ersetzen.
    let mut cells = cells;
    if s.cleanup {
        let snapshot: Vec<(bool, bool)> = cells.iter().map(|c| (c.fg, c.line)).collect();
        for cy in 0..ch {
            for cx in 0..cw {
                let i = (cy * cw + cx) as usize;
                if !snapshot[i].1 {
                    continue;
                }
                let mut line_nb = 0;
                let mut bg_nb = false;
                for (dx, dy) in N8 {
                    let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= cw as i32 || ny >= ch as i32 {
                        bg_nb = true;
                        continue;
                    }
                    let (fg, line) = snapshot[(ny as u32 * cw + nx as u32) as usize];
                    if !fg {
                        bg_nb = true;
                    } else if line {
                        line_nb += 1;
                    }
                }
                if line_nb == 0 && !bg_nb {
                    cells[i].line = false;
                    cells[i].lab = cells[i].body;
                }
            }
        }
    }

    // ---------- 4. Farbanpassung + Palette ----------
    // Für das Clustering wird die Farbigkeit (a/b) stärker gewichtet als die
    // Helligkeit – sonst verschmelzen ähnlich helle Farbtöne (z. B. Oliv und Khaki).
    const CHROMA_W: f32 = 2.0;
    let adjust = |c: Lab| Lab {
        l: ((c.l - 0.5) * s.contrast + 0.5).clamp(0.0, 1.0),
        a: c.a * s.saturation * CHROMA_W,
        b: c.b * s.saturation * CHROMA_W,
    };
    let adjusted: Vec<Lab> = cells.iter().map(|c| adjust(c.lab)).collect();
    let samples: Vec<Lab> = cells
        .iter()
        .zip(&adjusted)
        .filter(|(c, _)| c.fg)
        .map(|(_, l)| *l)
        .collect();
    let weighted_palette = kmeans(&samples, s.colors.max(2), 24);

    let mut index: Vec<Option<u16>> = cells
        .par_iter()
        .zip(adjusted.par_iter())
        .map(|(c, l)| c.fg.then(|| nearest(&weighted_palette, l) as u16))
        .collect();
    let palette: Vec<Lab> = weighted_palette
        .iter()
        .map(|p| Lab { l: p.l, a: p.a / CHROMA_W, b: p.b / CHROMA_W })
        .collect();

    // ---------- 5. Aufräumen ----------
    if s.cleanup {
        index = remove_orphans(&index, &cells, &palette, cw, ch);
    }

    // ---------- 6. Ausgabe + Außenkontur ----------
    // Ist das Quellbild an den Ecken transparent, bleibt die Ausgabe transparent
    let src_transparent = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
        .iter()
        .all(|&(x, y)| raw[((y * w + x) * 4 + 3) as usize] < 128);
    let bg_rgba = if s.transparent || src_transparent {
        Rgba([0, 0, 0, 0])
    } else {
        let c = bg_color.unwrap_or([255, 255, 255]);
        Rgba([c[0], c[1], c[2], 255])
    };
    let mut small = RgbaImage::new(cw, ch);
    for cy in 0..ch {
        for cx in 0..cw {
            let i = (cy * cw + cx) as usize;
            let px = match index[i] {
                None => bg_rgba,
                Some(p) => {
                    let mut lab = palette[p as usize];
                    if s.outline && touches_bg(&index, cx, cy, cw, ch) {
                        // Selective Outline: dunklere, leicht entsättigte Variante der eigenen Farbe
                        lab = Lab { l: (lab.l * 0.45).min(0.32), a: lab.a * 0.7, b: lab.b * 0.7 };
                    }
                    let [r, g, b] = lab_to_rgb(lab);
                    Rgba([r, g, b, 255])
                }
            };
            small.put_pixel(cx, cy, px);
        }
    }

    Output { small, pixel_size: block, palette_size: palette.len() }
}

/// Standardhöhe der Pixel-Art, wenn weder Breite, Höhe noch Blockgröße angegeben sind
pub const DEFAULT_HEIGHT: u32 = 160;

/// Berechnet die Rastergröße (Breite, Höhe in Pixeln) für einen Bildbereich.
/// Sind Breite und Höhe angegeben, wird das Seitenverhältnis erhalten und das
/// Raster passt in die angegebene Box.
fn grid_size(rw: u32, rh: u32, s: &Settings) -> (u32, u32) {
    let (rwf, rhf) = (rw as f32, rh as f32);
    let (cw, ch) = if s.pixel_size > 0 {
        (rw.div_ceil(s.pixel_size), rh.div_ceil(s.pixel_size))
    } else {
        match (s.width, s.height) {
            (Some(tw), Some(th)) => {
                let b = (rwf / tw as f32).max(rhf / th as f32);
                (((rwf / b).round() as u32).min(tw), ((rhf / b).round() as u32).min(th))
            }
            (Some(tw), None) => (tw, (rhf * tw as f32 / rwf).round() as u32),
            (None, th) => {
                let th = th.unwrap_or(DEFAULT_HEIGHT);
                ((rwf * th as f32 / rhf).round() as u32, th)
            }
        }
    };
    (cw.clamp(1, rw), ch.clamp(1, rh))
}

/// Dominante Farbe einer Zelle: 2-Means-Clustering, größerer Cluster gewinnt.
/// Dadurch entstehen harte Kanten statt Mischfarben.
fn dominant(px: &[Lab]) -> Lab {
    if px.is_empty() {
        return Lab::default();
    }
    let mean = px.iter().fold(Lab::default(), |mut a, c| { a.add(c); a }).scale(1.0 / px.len() as f32);
    if px.len() < 4 {
        return mean;
    }
    let mut c0 = *px.iter().min_by(|a, b| a.l.total_cmp(&b.l)).unwrap();
    let mut c1 = *px.iter().max_by(|a, b| a.l.total_cmp(&b.l)).unwrap();
    if c0.dist2(&c1) < 0.03 * 0.03 {
        return mean;
    }
    let (mut n0, mut n1) = (0usize, 0usize);
    for _ in 0..5 {
        let (mut s0, mut s1) = (Lab::default(), Lab::default());
        n0 = 0;
        n1 = 0;
        for p in px {
            if p.dist2(&c0) <= p.dist2(&c1) {
                s0.add(p);
                n0 += 1;
            } else {
                s1.add(p);
                n1 += 1;
            }
        }
        if n0 > 0 { c0 = s0.scale(1.0 / n0 as f32); }
        if n1 > 0 { c1 = s1.scale(1.0 / n1 as f32); }
    }
    if c0.dist2(&c1) < 0.04 * 0.04 {
        mean
    } else if n0 >= n1 {
        c0
    } else {
        c1
    }
}

fn nearest(palette: &[Lab], c: &Lab) -> usize {
    let mut best = 0;
    let mut bd = f32::MAX;
    for (i, p) in palette.iter().enumerate() {
        let d = p.dist2(c);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// k-Means mit deterministischer k-Means++-Initialisierung.
fn kmeans(data: &[Lab], k: usize, iters: usize) -> Vec<Lab> {
    if data.is_empty() {
        return vec![Lab { l: 1.0, a: 0.0, b: 0.0 }];
    }
    let k = k.min(data.len());
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rand = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };

    // k-Means++
    let mut centers = vec![data[data.len() / 2]];
    let mut d2: Vec<f32> = data.iter().map(|p| p.dist2(&centers[0])).collect();
    while centers.len() < k {
        let total: f64 = d2.iter().map(|&d| d as f64).sum();
        if total <= 0.0 {
            break;
        }
        let mut t = rand() * total;
        let mut pick = data.len() - 1;
        for (i, &d) in d2.iter().enumerate() {
            t -= d as f64;
            if t <= 0.0 {
                pick = i;
                break;
            }
        }
        let c = data[pick];
        centers.push(c);
        d2.par_iter_mut().zip(data.par_iter()).for_each(|(d, p)| *d = d.min(p.dist2(&c)));
    }

    // Lloyd-Iterationen
    for _ in 0..iters {
        let k = centers.len();
        let (sums, counts) = data
            .par_iter()
            .fold(
                || (vec![Lab::default(); k], vec![0u32; k]),
                |(mut s, mut n), p| {
                    let i = nearest(&centers, p);
                    s[i].add(p);
                    n[i] += 1;
                    (s, n)
                },
            )
            .reduce(
                || (vec![Lab::default(); k], vec![0u32; k]),
                |(mut s, mut n), (s2, n2)| {
                    for i in 0..k {
                        s[i].add(&s2[i]);
                        n[i] += n2[i];
                    }
                    (s, n)
                },
            );
        let mut moved = 0f32;
        for i in 0..k {
            if counts[i] > 0 {
                let c = sums[i].scale(1.0 / counts[i] as f32);
                moved = moved.max(c.dist2(&centers[i]));
                centers[i] = c;
            }
        }
        if moved < 1e-8 {
            break;
        }
    }
    centers
}

const N8: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// Ersetzt Einzelpixel ohne gleichfarbigen Nachbarn durch die häufigste
/// Nachbarfarbe – sofern diese farblich nicht zu weit entfernt ist.
/// Linienpixel bleiben unangetastet (Augen, Details).
fn remove_orphans(index: &[Option<u16>], cells: &[Cell], palette: &[Lab], cw: u32, ch: u32) -> Vec<Option<u16>> {
    let mut out = index.to_vec();
    for cy in 0..ch {
        for cx in 0..cw {
            let i = (cy * cw + cx) as usize;
            let Some(me) = index[i] else { continue };
            if cells[i].line {
                continue;
            }
            let mut counts: Vec<(u16, u8)> = Vec::with_capacity(8);
            let mut same = false;
            for (dx, dy) in N8 {
                let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                if nx < 0 || ny < 0 || nx >= cw as i32 || ny >= ch as i32 {
                    continue;
                }
                let Some(p) = index[(ny as u32 * cw + nx as u32) as usize] else { continue };
                if p == me {
                    same = true;
                    break;
                }
                match counts.iter_mut().find(|(c, _)| *c == p) {
                    Some(e) => e.1 += 1,
                    None => counts.push((p, 1)),
                }
            }
            if same {
                continue;
            }
            if let Some(&(best, n)) = counts.iter().max_by_key(|(_, n)| *n) {
                if n >= 3 && palette[best as usize].dist2(&palette[me as usize]) < 0.12 * 0.12 {
                    out[i] = Some(best);
                }
            }
        }
    }
    out
}

fn touches_bg(index: &[Option<u16>], cx: u32, cy: u32, cw: u32, ch: u32) -> bool {
    for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
        if nx < 0 || ny < 0 || nx >= cw as i32 || ny >= ch as i32 {
            continue;
        }
        if index[(ny as u32 * cw + nx as u32) as usize].is_none() {
            return true;
        }
    }
    false
}

/// Erkennt einen einfarbigen Hintergrund: sind alle vier Ecken ähnlich gefärbt,
/// wird von allen Randpixeln dieser Farbe aus geflutet.
fn detect_background(img: &RgbaImage) -> (Option<Vec<bool>>, Option<[u8; 3]>) {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    let px = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [raw[i] as i32, raw[i + 1] as i32, raw[i + 2] as i32]
    };
    // Transparente Ecken: Alpha-Kanal ist bereits die Hintergrundmaske
    // (ein Flood-Fill über die RGB-Werte könnte sonst in dunkle Bildteile auslaufen).
    if [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
        .iter()
        .any(|&(x, y)| raw[((y * w + x) * 4 + 3) as usize] < 128)
    {
        return (None, None);
    }
    let corners = [px(0, 0), px(w - 1, 0), px(0, h - 1), px(w - 1, h - 1)];
    let mut avg = [0i32; 3];
    for c in &corners {
        for k in 0..3 {
            avg[k] += c[k];
        }
    }
    let avg = avg.map(|v| v / 4);
    let d2 = |a: [i32; 3], b: [i32; 3]| (0..3).map(|k| (a[k] - b[k]).pow(2)).sum::<i32>();
    if corners.iter().any(|c| d2(*c, avg) > 18 * 18) {
        return (None, None);
    }
    const TOL: i32 = 22 * 22;
    let mut mask = vec![false; (w * h) as usize];
    let mut stack: Vec<u32> = Vec::new();
    let seed = |x: u32, y: u32, mask: &mut Vec<bool>, stack: &mut Vec<u32>| {
        let i = y * w + x;
        if !mask[i as usize] && d2(px(x, y), avg) <= TOL {
            mask[i as usize] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        seed(x, 0, &mut mask, &mut stack);
        seed(x, h - 1, &mut mask, &mut stack);
    }
    for y in 0..h {
        seed(0, y, &mut mask, &mut stack);
        seed(w - 1, y, &mut mask, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        if x > 0 { seed(x - 1, y, &mut mask, &mut stack); }
        if x + 1 < w { seed(x + 1, y, &mut mask, &mut stack); }
        if y > 0 { seed(x, y - 1, &mut mask, &mut stack); }
        if y + 1 < h { seed(x, y + 1, &mut mask, &mut stack); }
    }
    let bg_count = mask.iter().filter(|&&b| b).count();
    // Nur sinnvoll, wenn der Hintergrund einen nennenswerten Teil ausmacht
    if bg_count * 10 < mask.len() {
        return (None, None);
    }
    (Some(mask), Some([avg[0] as u8, avg[1] as u8, avg[2] as u8]))
}

fn content_bbox(w: u32, h: u32, is_bg: &(impl Fn(u32, u32) -> bool + Sync)) -> Option<(u32, u32, u32, u32)> {
    let rows: Vec<(u32, u32)> = (0..h)
        .into_par_iter()
        .filter_map(|y| {
            let first = (0..w).find(|&x| !is_bg(x, y))?;
            let last = (0..w).rev().find(|&x| !is_bg(x, y))?;
            Some((first, last))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let y0 = (0..h).find(|&y| (0..w).any(|x| !is_bg(x, y)))?;
    let y1 = (0..h).rev().find(|&y| (0..w).any(|x| !is_bg(x, y)))?;
    let x0 = rows.iter().map(|r| r.0).min()?;
    let x1 = rows.iter().map(|r| r.1).max()?;
    Some((x0, y0, x1 + 1, y1 + 1))
}
