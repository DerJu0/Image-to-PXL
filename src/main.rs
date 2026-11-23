use anyhow::{bail, Context, Result};
use clap::Parser;
use image_to_pxl::{is_image, pad_to, pixelate, upscale, Settings};
use std::path::PathBuf;
use std::time::Instant;

/// Wandelt Bilder in Pixel-Art um.
///
/// Beispiel:  pxl Testdaten -o Ausgabe --rows 160 --colors 24
#[derive(Parser, Debug)]
#[command(name = "pxl", version, about)]
struct Cli {
    /// Eingabedateien oder -ordner (PNG, JPG, WEBP, BMP)
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Ausgabeordner
    #[arg(short, long, default_value = "Ausgabe")]
    output: PathBuf,

    /// Auflösung der Pixel-Art, z. B. "64x64", "128x" (nur Breite) oder "x160" (nur Höhe).
    /// Das Seitenverhältnis bleibt erhalten; das Bild passt in die angegebene Box.
    #[arg(short = 'R', long, value_parser = parse_res, conflicts_with_all = ["width", "height"])]
    res: Option<(Option<u32>, Option<u32>)>,

    /// Breite der Pixel-Art in Pixeln
    #[arg(short = 'W', long)]
    width: Option<u32>,

    /// Höhe der Pixel-Art in Pixeln (Standard: 160, wenn nichts angegeben)
    #[arg(short = 'H', long, visible_alias = "rows", short_alias = 'r')]
    height: Option<u32>,

    /// Mit Hintergrund auf exakt die angegebene Breite × Höhe auffüllen (Motiv zentriert)
    #[arg(long)]
    pad: bool,

    /// Feste Blockgröße in Originalpixeln (überschreibt Auflösungsangaben)
    #[arg(short = 'p', long, default_value_t = 0)]
    pixel_size: u32,

    /// Anzahl Farben der Palette
    #[arg(short, long, default_value_t = 24)]
    colors: usize,

    /// Vergrößerungsfaktor der Ausgabe (0 = ungefähr Originalgröße)
    #[arg(short, long, default_value_t = 0)]
    scale: u32,

    /// Zusätzlich die kleine 1:1-Version speichern (*_pxl_1x.png)
    #[arg(long)]
    small: bool,

    /// Hintergrund transparent machen
    #[arg(short, long)]
    transparent: bool,

    /// Auf den Bildinhalt zuschneiden (entfernt leeren Hintergrund)
    #[arg(long)]
    crop: bool,

    /// Keine automatische Hintergrunderkennung
    #[arg(long)]
    no_bg: bool,

    /// Keine dunkle Außenkontur um die Figur
    #[arg(long)]
    no_outline: bool,

    /// Isolierte Einzelpixel nicht entfernen
    #[arg(long)]
    no_cleanup: bool,

    /// Sättigungsfaktor (1.0 = unverändert)
    #[arg(long, default_value_t = 1.15)]
    saturation: f32,

    /// Kontrastfaktor (1.0 = unverändert)
    #[arg(long, default_value_t = 1.05)]
    contrast: f32,

    /// Empfindlichkeit der Linienerkennung (kleiner = mehr Linien)
    #[arg(long, default_value_t = 0.14)]
    line_delta: f32,

    /// Anteil dunkler Linienpixel in einem Block, ab dem er zur Kontur wird (0..1)
    #[arg(long, default_value_t = 0.30)]
    line_threshold: f32,
}

/// Parst "BxH", "Bx", "xH" oder "H".
fn parse_res(s: &str) -> Result<(Option<u32>, Option<u32>), String> {
    let num = |t: &str| -> Result<Option<u32>, String> {
        let t = t.trim();
        if t.is_empty() {
            return Ok(None);
        }
        match t.parse::<u32>() {
            Ok(0) | Err(_) => Err(format!("ungültige Zahl: \"{t}\"")),
            Ok(v) => Ok(Some(v)),
        }
    };
    let lower = s.to_ascii_lowercase();
    let (w, h) = match lower.split_once(['x', '*', '×']) {
        Some((a, b)) => (num(a)?, num(b)?),
        None => (None, num(&lower)?),
    };
    if w.is_none() && h.is_none() {
        return Err("Format: 64x64, 128x oder x160".into());
    }
    Ok((w, h))
}

fn collect_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for p in inputs {
        if p.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(p)
                .with_context(|| format!("Ordner nicht lesbar: {}", p.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| f.is_file() && is_image(f))
                .collect();
            entries.sort();
            files.extend(entries);
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            bail!("Nicht gefunden: {}", p.display());
        }
    }
    Ok(files)
}

/// Ohne Argumente (z. B. Doppelklick im Explorer) wird die GUI gestartet,
/// statt dass sich nur kurz ein Konsolenfenster öffnet und wieder schließt.
fn launch_gui_if_no_args() {
    if std::env::args_os().len() > 1 {
        return;
    }
    let gui = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("pxl-gui.exe")))
        .filter(|p| p.is_file());
    if let Some(gui) = gui {
        if std::process::Command::new(&gui).spawn().is_ok() {
            std::process::exit(0);
        }
    }
    println!("pxl ist das Kommandozeilenprogramm. Beispiel:\n");
    println!("  pxl.exe Testdaten -o Ausgabe --crop --res x128\n");
    println!("Alle Optionen: pxl.exe --help");
    println!("Die grafische Oberfläche ist pxl-gui.exe.\n");
    println!("Enter drücken zum Beenden …");
    let _ = std::io::stdin().read_line(&mut String::new());
    std::process::exit(0);
}

fn main() -> Result<()> {
    launch_gui_if_no_args();
    let cli = Cli::parse();
    let files = collect_inputs(&cli.inputs)?;
    if files.is_empty() {
        bail!("Keine Bilddateien gefunden.");
    }
    std::fs::create_dir_all(&cli.output)
        .with_context(|| format!("Ausgabeordner nicht anlegbar: {}", cli.output.display()))?;

    let (width, height) = cli.res.unwrap_or((cli.width, cli.height));
    let settings = Settings {
        pixel_size: cli.pixel_size,
        width,
        height,
        colors: cli.colors,
        detect_background: !cli.no_bg,
        transparent: cli.transparent,
        crop: cli.crop,
        outline: !cli.no_outline,
        cleanup: !cli.no_cleanup,
        saturation: cli.saturation,
        contrast: cli.contrast,
        line_delta: cli.line_delta,
        line_threshold: cli.line_threshold,
    };

    for file in &files {
        let t = Instant::now();
        let img = image::open(file)
            .with_context(|| format!("Bild nicht lesbar: {}", file.display()))?
            .to_rgba8();
        let mut out = pixelate(&img, &settings);
        if cli.pad {
            if let (Some(tw), Some(th)) = (width, height) {
                out.small = pad_to(&out.small, tw, th);
            } else {
                eprintln!("Hinweis: --pad braucht Breite und Höhe (z. B. --res 64x64)");
            }
        }

        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
        let (sw, sh) = out.small.dimensions();
        let big = upscale(&out.small, cli.scale, out.pixel_size);
        let scale = big.width() / sw;
        let target = cli.output.join(format!("{stem}_pxl.png"));
        big.save(&target).with_context(|| format!("Speichern fehlgeschlagen: {}", target.display()))?;
        if cli.small {
            let t1 = cli.output.join(format!("{stem}_pxl_1x.png"));
            out.small.save(&t1)?;
        }
        println!(
            "✔ {}  →  {}×{} Pixel, Block {:.1} px, {} Farben, Ausgabe {}×{} ({:.1} s)",
            file.file_name().unwrap_or_default().to_string_lossy(),
            sw,
            sh,
            out.pixel_size,
            out.palette_size,
            sw * scale,
            sh * scale,
            t.elapsed().as_secs_f32()
        );
    }
    Ok(())
}
