//! Grafische Oberfläche für Image to PXL.
//!
//! Live-Vorschau: Jede Änderung der Einstellungen rendert die Pixel-Art im
//! Hintergrund neu (mit kurzer Entprellung), die Oberfläche bleibt flüssig.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use image::imageops::FilterType;
use image::RgbaImage;
use image_to_pxl::{is_image, pad_to, pixelate, upscale, Settings};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(150);
const PREVIEW_MAX: u32 = 2048;

/// Tastenkürzel-Bezeichnung je nach System (Strg unter Windows/Linux, Cmd auf dem Mac)
const MOD_KEY: &str = if cfg!(target_os = "macos") { "Cmd" } else { "Strg" };

/// Ohne Konsole wären Fehler unsichtbar – daher als Dialog anzeigen.
fn show_error(msg: &str) {
    eprintln!("{msg}");
    // macOS erlaubt Dialoge nur aus dem Hauptthread
    if cfg!(target_os = "macos") && std::thread::current().name() != Some("main") {
        return;
    }
    rfd::MessageDialog::new()
        .set_title("Image to PXL – Fehler")
        .set_description(msg)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        show_error(&format!("Unerwarteter Fehler:\n{info}"));
    }));
    if let Err(e) = run() {
        show_error(&format!(
            "Das Programm konnte nicht gestartet werden:\n{e}\n\n\
             Mögliche Ursache: Grafiktreiber ohne OpenGL-2-Unterstützung."
        ));
    }
}

fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Image to PXL")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true)
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native("Image to PXL", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

// ------------------------------------------------------------------ Parameter

#[derive(Clone, Copy, PartialEq, Debug)]
enum ResMode {
    Height,
    Width,
    Box,
    Block,
}

impl ResMode {
    fn label(self) -> &'static str {
        match self {
            ResMode::Height => "Höhe festlegen",
            ResMode::Width => "Breite festlegen",
            ResMode::Box => "Breite × Höhe",
            ResMode::Block => "Blockgröße",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Params {
    mode: ResMode,
    width: u32,
    height: u32,
    block: u32,
    pad: bool,
    colors: usize,
    crop: bool,
    outline: bool,
    cleanup: bool,
    transparent: bool,
    detect_bg: bool,
    saturation: f32,
    contrast: f32,
    line_delta: f32,
    line_threshold: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            mode: ResMode::Height,
            width: 128,
            height: 160,
            block: 16,
            pad: false,
            colors: 24,
            crop: true,
            outline: true,
            cleanup: true,
            transparent: false,
            detect_bg: true,
            saturation: 1.15,
            contrast: 1.05,
            line_delta: 0.14,
            line_threshold: 0.30,
        }
    }
}

impl Params {
    fn settings(&self) -> Settings {
        let (width, height, pixel_size) = match self.mode {
            ResMode::Height => (None, Some(self.height), 0),
            ResMode::Width => (Some(self.width), None, 0),
            ResMode::Box => (Some(self.width), Some(self.height), 0),
            ResMode::Block => (None, None, self.block),
        };
        Settings {
            pixel_size,
            width,
            height,
            colors: self.colors,
            detect_background: self.detect_bg,
            transparent: self.transparent,
            crop: self.crop,
            outline: self.outline,
            cleanup: self.cleanup,
            saturation: self.saturation,
            contrast: self.contrast,
            line_delta: self.line_delta,
            line_threshold: self.line_threshold,
        }
    }
}

/// Komplette Umwandlung inkl. optionalem Auffüllen
fn render(img: &RgbaImage, p: &Params) -> Rendered {
    let out = pixelate(img, &p.settings());
    let small = if p.pad && p.mode == ResMode::Box {
        pad_to(&out.small, p.width, p.height)
    } else {
        out.small
    };
    Rendered { small, pixel_size: out.pixel_size, palette: out.palette_size }
}

struct Rendered {
    small: RgbaImage,
    pixel_size: f32,
    palette: usize,
}

// ------------------------------------------------------------- Nachrichten

enum Msg {
    Loaded { path: PathBuf, image: Arc<RgbaImage>, preview: ColorImage },
    Rendered { generation: u64, result: Rendered, ms: u128 },
    Saved(PathBuf),
    BatchProgress { done: usize, total: usize, current: String },
    BatchDone { count: usize, out: PathBuf, errors: Vec<String> },
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Pixel,
    Original,
    Split,
}

// ------------------------------------------------- Speicher-Einstellungen

#[derive(Clone, Copy, PartialEq, Debug)]
enum SaveLoc {
    /// Im selben Ordner wie das Originalbild
    NextToSource,
    /// In einem selbst gewählten Ordner
    Folder,
}

/// Export-Einstellungen; werden dauerhaft gespeichert:
/// Windows `%APPDATA%\Image to PXL\`, macOS `~/Library/Application Support/Image to PXL/`,
/// Linux `~/.config/Image to PXL/`.
#[derive(Clone, PartialEq, Debug)]
struct Prefs {
    loc: SaveLoc,
    dir: Option<PathBuf>,
    /// Vor jedem Speichern nachfragen (Dateidialog) statt direkt zu speichern
    ask: bool,
    scale: u32,
    small: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { loc: SaveLoc::NextToSource, dir: None, ask: true, scale: 0, small: false }
    }
}

impl Prefs {
    fn file() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        base.map(|d| d.join("Image to PXL").join("einstellungen.txt"))
    }

    fn load() -> Self {
        let mut p = Prefs::default();
        let Some(text) = Prefs::file().and_then(|f| std::fs::read_to_string(f).ok()) else { return p };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "speicherort" => p.loc = if v == "ordner" { SaveLoc::Folder } else { SaveLoc::NextToSource },
                "ordner" if !v.is_empty() => p.dir = Some(PathBuf::from(v)),
                "nachfragen" => p.ask = v == "ja",
                "vergroesserung" => p.scale = v.parse().unwrap_or(0),
                "klein" => p.small = v == "ja",
                _ => {}
            }
        }
        p
    }

    fn save(&self) {
        let Some(f) = Prefs::file() else { return };
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let yn = |b: bool| if b { "ja" } else { "nein" };
        let text = format!(
            "speicherort={}\nordner={}\nnachfragen={}\nvergroesserung={}\nklein={}\n",
            if self.loc == SaveLoc::Folder { "ordner" } else { "original" },
            self.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            yn(self.ask),
            self.scale,
            yn(self.small),
        );
        let _ = std::fs::write(f, text);
    }

    /// Zielordner für ein Bild; `None`, wenn kein Ordner bestimmt werden kann
    fn target_dir(&self, source: Option<&Path>) -> Option<PathBuf> {
        match self.loc {
            SaveLoc::Folder => self.dir.clone(),
            SaveLoc::NextToSource => source.and_then(|p| p.parent()).map(Path::to_path_buf),
        }
    }
}

/// Liefert einen noch nicht vorhandenen Dateinamen (name.png, name_2.png, …)
fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.png"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}_{n}.png"));
        n += 1;
    }
    p
}

// --------------------------------------------------------------------- App

struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    params: Params,
    last_params: Params,
    view: View,
    prefs: Prefs,
    saved_prefs: Prefs,

    path: Option<PathBuf>,
    source: Option<Arc<RgbaImage>>,
    orig_tex: Option<TextureHandle>,
    pix_tex: Option<TextureHandle>,
    result: Option<Rendered>,

    generation: u64,
    dirty_since: Option<Instant>,
    rendering: bool,
    loading: bool,
    saving: bool,
    batch: Option<(usize, usize, String)>,
    status: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tx, rx) = channel();
        let params = Params::default();
        let prefs = Prefs::load();
        let mut app = Self {
            tx,
            rx,
            last_params: params.clone(),
            params,
            view: View::Split,
            prefs: prefs.clone(),
            saved_prefs: prefs,
            path: None,
            source: None,
            orig_tex: None,
            pix_tex: None,
            result: None,
            generation: 0,
            dirty_since: None,
            rendering: false,
            loading: false,
            saving: false,
            batch: None,
            status: "Bild öffnen oder per Drag & Drop hineinziehen.".into(),
        };
        // Bild als Startargument (z. B. „Öffnen mit“ im Explorer)
        if let Some(p) = std::env::args_os().nth(1).map(PathBuf::from).filter(|p| p.is_file()) {
            app.load(p, &cc.egui_ctx);
        }
        app
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Bild öffnen")
            .add_filter("Bilder", &image_to_pxl::EXTS)
            .pick_file()
        {
            self.load(p, ctx);
        }
    }

    fn load(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.loading = true;
        self.status = format!("Lade {} …", file_name(&path));
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match image::open(&path) {
                Ok(img) => {
                    let img = img.to_rgba8();
                    let (w, h) = img.dimensions();
                    let f = (PREVIEW_MAX as f32 / w.max(h) as f32).min(1.0);
                    let prev = if f < 1.0 {
                        image::imageops::resize(
                            &img,
                            ((w as f32 * f) as u32).max(1),
                            ((h as f32 * f) as u32).max(1),
                            FilterType::Triangle,
                        )
                    } else {
                        img.clone()
                    };
                    let preview = to_color_image(&prev);
                    Msg::Loaded { path, image: Arc::new(img), preview }
                }
                Err(e) => Msg::Error(format!("Bild nicht lesbar: {e}")),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn start_render(&mut self, ctx: &egui::Context) {
        let Some(src) = self.source.clone() else { return };
        self.generation += 1;
        self.rendering = true;
        self.dirty_since = None;
        let generation = self.generation;
        let params = self.params.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let result = render(&src, &params);
            let _ = tx.send(Msg::Rendered { generation, result, ms: t.elapsed().as_millis() });
            ctx.request_repaint();
        });
    }

    fn save_dialog(&mut self, ctx: &egui::Context) {
        let Some(res) = &self.result else { return };
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "bild".into());
        let dir = self.prefs.target_dir(self.path.as_deref());
        let target = match (&dir, self.prefs.ask) {
            // Direkt speichern in den eingestellten Ordner (ohne Überschreiben)
            (Some(dir), false) => unique_path(dir, &format!("{stem}_pxl")),
            // Dateidialog, startet im eingestellten Ordner
            _ => {
                let mut dlg = rfd::FileDialog::new()
                    .set_title("Pixel-Art speichern")
                    .add_filter("PNG", &["png"])
                    .set_file_name(format!("{stem}_pxl.png"));
                if let Some(d) = dir.as_ref().filter(|d| d.is_dir()) {
                    dlg = dlg.set_directory(d);
                }
                let Some(t) = dlg.save_file() else { return };
                t.with_extension("png")
            }
        };
        let small = res.small.clone();
        let pixel_size = res.pixel_size;
        let scale = self.prefs.scale;
        let also_small = self.prefs.small;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.saving = true;
        std::thread::spawn(move || {
            if let Some(d) = target.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let big = upscale(&small, scale, pixel_size);
            let mut r = big.save(&target);
            if r.is_ok() && also_small {
                let stem = target.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                r = small.save(target.with_file_name(format!("{stem}_1x.png")));
            }
            let _ = tx.send(match r {
                Ok(()) => Msg::Saved(target),
                Err(e) => Msg::Error(format!("Speichern fehlgeschlagen: {e}")),
            });
            ctx.request_repaint();
        });
    }

    fn batch_dialog(&mut self, ctx: &egui::Context) {
        let Some(input) = rfd::FileDialog::new().set_title("Eingabeordner wählen").pick_folder() else {
            return;
        };
        let configured = match self.prefs.loc {
            SaveLoc::NextToSource => Some(input.clone()),
            SaveLoc::Folder => self.prefs.dir.clone(),
        };
        let output = match (configured, self.prefs.ask) {
            (Some(dir), false) => dir,
            (start, _) => {
                let start = start.filter(|d| d.is_dir()).unwrap_or_else(|| input.clone());
                let Some(dir) = rfd::FileDialog::new()
                    .set_title("Ausgabeordner wählen")
                    .set_directory(&start)
                    .pick_folder()
                else {
                    return;
                };
                dir
            }
        };
        if let Err(e) = std::fs::create_dir_all(&output) {
            self.status = format!("Ausgabeordner nicht anlegbar: {e}");
            return;
        }
        let files: Vec<PathBuf> = match std::fs::read_dir(&input) {
            Ok(rd) => {
                let mut v: Vec<PathBuf> =
                    rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file() && is_image(p)).collect();
                v.sort();
                v
            }
            Err(e) => {
                self.status = format!("Ordner nicht lesbar: {e}");
                return;
            }
        };
        if files.is_empty() {
            self.status = "Im gewählten Ordner sind keine Bilder.".into();
            return;
        }
        let params = self.params.clone();
        let scale = self.prefs.scale;
        let also_small = self.prefs.small;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.batch = Some((0, files.len(), String::new()));
        std::thread::spawn(move || {
            let total = files.len();
            let mut errors = Vec::new();
            for (i, f) in files.iter().enumerate() {
                let _ = tx.send(Msg::BatchProgress { done: i, total, current: file_name(f) });
                ctx.request_repaint();
                let res = (|| -> Result<(), String> {
                    let img = image::open(f).map_err(|e| e.to_string())?.to_rgba8();
                    let r = render(&img, &params);
                    let stem = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    upscale(&r.small, scale, r.pixel_size)
                        .save(output.join(format!("{stem}_pxl.png")))
                        .map_err(|e| e.to_string())?;
                    if also_small {
                        r.small.save(output.join(format!("{stem}_pxl_1x.png"))).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })();
                if let Err(e) = res {
                    errors.push(format!("{}: {e}", file_name(f)));
                }
            }
            let _ = tx.send(Msg::BatchDone { count: total - errors.len(), out: output, errors });
            ctx.request_repaint();
        });
    }

    fn handle_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded { path, image, preview } => {
                    self.loading = false;
                    self.orig_tex = Some(ctx.load_texture("original", preview, TextureOptions::LINEAR));
                    self.pix_tex = None;
                    self.result = None;
                    self.status = format!("{} geladen ({}×{})", file_name(&path), image.width(), image.height());
                    self.source = Some(image);
                    self.path = Some(path);
                    self.dirty_since = Some(Instant::now() - DEBOUNCE);
                }
                Msg::Rendered { generation, result, ms } => {
                    self.rendering = false;
                    if generation == self.generation {
                        let (w, h) = result.small.dimensions();
                        self.pix_tex = Some(ctx.load_texture(
                            "pixel",
                            to_color_image(&result.small),
                            TextureOptions::NEAREST,
                        ));
                        self.status = format!(
                            "Pixel-Art {w}×{h} · Block {:.1} px · {} Farben · {ms} ms",
                            result.pixel_size, result.palette
                        );
                        self.result = Some(result);
                    }
                }
                Msg::Saved(p) => {
                    self.saving = false;
                    self.status = format!("Gespeichert: {}", p.display());
                }
                Msg::BatchProgress { done, total, current } => {
                    self.batch = Some((done, total, current));
                }
                Msg::BatchDone { count, out, errors } => {
                    self.batch = None;
                    self.status = if errors.is_empty() {
                        format!("Stapel fertig: {count} Bilder nach {} gespeichert", out.display())
                    } else {
                        format!("Stapel fertig: {count} ok, {} Fehler – {}", errors.len(), errors.join("; "))
                    };
                }
                Msg::Error(e) => {
                    self.loading = false;
                    self.saving = false;
                    self.status = e;
                }
            }
        }
    }

    // ----------------------------------------------------------- UI-Teile

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let p = &mut self.params;

        ui.heading("Auflösung");
        egui::ComboBox::from_id_salt("resmode")
            .selected_text(p.mode.label())
            .width(200.0)
            .show_ui(ui, |ui| {
                for m in [ResMode::Height, ResMode::Width, ResMode::Box, ResMode::Block] {
                    ui.selectable_value(&mut p.mode, m, m.label());
                }
            });
        ui.add_space(4.0);
        egui::Grid::new("res").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            if matches!(p.mode, ResMode::Width | ResMode::Box) {
                ui.label("Breite");
                ui.add(egui::DragValue::new(&mut p.width).range(4..=2048).suffix(" px").speed(1.0));
                ui.end_row();
            }
            if matches!(p.mode, ResMode::Height | ResMode::Box) {
                ui.label("Höhe");
                ui.add(egui::DragValue::new(&mut p.height).range(4..=2048).suffix(" px").speed(1.0));
                ui.end_row();
            }
            if p.mode == ResMode::Block {
                ui.label("Block");
                ui.add(egui::DragValue::new(&mut p.block).range(1..=512).suffix(" px").speed(0.2));
                ui.end_row();
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Schnellwahl:");
            for v in [32u32, 64, 96, 128, 160, 256] {
                if ui.small_button(v.to_string()).clicked() {
                    match p.mode {
                        ResMode::Width => p.width = v,
                        ResMode::Box => {
                            p.width = v;
                            p.height = v;
                        }
                        _ => p.height = v,
                    }
                    if p.mode == ResMode::Block {
                        p.mode = ResMode::Height;
                    }
                }
            }
        });
        if p.mode == ResMode::Box {
            ui.checkbox(&mut p.pad, "Auf exakte Größe auffüllen")
                .on_hover_text("Motiv zentriert, Rest mit Hintergrund gefüllt");
        }
        ui.checkbox(&mut p.crop, "Auf Motiv zuschneiden")
            .on_hover_text("Leeren Hintergrund entfernen – die Auflösung bezieht sich dann auf die Figur");

        ui.separator();
        ui.heading("Farben");
        ui.add(egui::Slider::new(&mut p.colors, 2..=64).text("Palette"));
        ui.add(egui::Slider::new(&mut p.saturation, 0.0..=2.0).text("Sättigung"));
        ui.add(egui::Slider::new(&mut p.contrast, 0.5..=1.8).text("Kontrast"));

        ui.separator();
        ui.heading("Stil");
        ui.checkbox(&mut p.outline, "Außenkontur");
        ui.checkbox(&mut p.cleanup, "Einzelpixel aufräumen");
        ui.checkbox(&mut p.detect_bg, "Hintergrund erkennen");
        ui.add_enabled_ui(p.detect_bg, |ui| {
            ui.checkbox(&mut p.transparent, "Hintergrund transparent");
        });

        egui::CollapsingHeader::new("Linienerkennung").default_open(false).show(ui, |ui| {
            ui.add(egui::Slider::new(&mut p.line_delta, 0.02..=0.4).text("Empfindlichkeit"))
                .on_hover_text("Kleiner = mehr dunkle Linien werden erkannt");
            ui.add(egui::Slider::new(&mut p.line_threshold, 0.05..=0.9).text("Linienanteil"))
                .on_hover_text("Anteil dunkler Pixel pro Block, ab dem er zur Kontur wird");
        });

        ui.add_space(6.0);
        if ui.button("Standardwerte").clicked() {
            *p = Params::default();
        }

        ui.separator();
        ui.heading("Speichern");
        let pr = &mut self.prefs;
        ui.label("Speicherort");
        ui.radio_value(&mut pr.loc, SaveLoc::NextToSource, "Neben dem Originalbild");
        ui.radio_value(&mut pr.loc, SaveLoc::Folder, "Eigener Ordner:");
        if pr.loc == SaveLoc::Folder {
            ui.horizontal(|ui| {
                let text = pr.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "– kein Ordner gewählt –".into());
                ui.add(egui::Label::new(egui::RichText::new(&text).monospace().size(11.0)).truncate())
                    .on_hover_text(&text);
            });
            ui.horizontal(|ui| {
                if ui.button("Ordner wählen…").clicked() {
                    let mut dlg = rfd::FileDialog::new().set_title("Speicherort für Pixel-Art wählen");
                    if let Some(d) = pr.dir.as_ref().filter(|d| d.is_dir()) {
                        dlg = dlg.set_directory(d);
                    }
                    if let Some(d) = dlg.pick_folder() {
                        pr.dir = Some(d);
                    }
                }
                if let Some(d) = pr.dir.clone() {
                    let (label, cmd) = if cfg!(windows) {
                        ("Im Explorer öffnen", "explorer")
                    } else if cfg!(target_os = "macos") {
                        ("Im Finder öffnen", "open")
                    } else {
                        ("Ordner öffnen", "xdg-open")
                    };
                    if ui.button(label).clicked() {
                        let _ = std::fs::create_dir_all(&d);
                        let _ = std::process::Command::new(cmd).arg(&d).spawn();
                    }
                }
            });
        }
        ui.checkbox(&mut pr.ask, "Vor dem Speichern nachfragen")
            .on_hover_text("Aus: „Speichern“ legt die Datei sofort im Speicherort ab (vorhandene Dateien werden nicht überschrieben)");
        if pr.loc == SaveLoc::Folder && pr.dir.is_none() && !pr.ask {
            ui.colored_label(Color32::from_rgb(230, 160, 60), "Bitte einen Ordner wählen – bis dahin wird nachgefragt.");
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Vergrößerung");
            ui.add(
                egui::DragValue::new(&mut pr.scale)
                    .range(0..=64)
                    .custom_formatter(|v, _| if v == 0.0 { "auto".into() } else { format!("{v}×") }),
            );
        });
        if let Some(r) = &self.result {
            let (w, h) = r.small.dimensions();
            let s = if pr.scale > 0 { pr.scale } else { (r.pixel_size.round() as u32).max(1) };
            ui.weak(format!("Datei: {}×{} px", w * s, h * s));
        }
        ui.checkbox(&mut pr.small, "Zusätzlich 1:1-Version speichern");
    }

    fn central_ui(&mut self, ui: &mut egui::Ui) {
        if self.source.is_none() {
            ui.centered_and_justified(|ui| {
                if self.loading {
                    ui.spinner();
                } else {
                    ui.label(
                        egui::RichText::new("Bild hierher ziehen oder oben auf „Bild öffnen…“ klicken")
                            .size(18.0)
                            .weak(),
                    );
                }
            });
            return;
        }
        let avail = ui.available_rect_before_wrap();
        match self.view {
            View::Original => image_panel(ui, avail, self.orig_tex.as_ref(), "Original"),
            View::Pixel => image_panel(ui, avail, self.pix_tex.as_ref(), "Pixel-Art"),
            View::Split => {
                let half = (avail.width() - 8.0) / 2.0;
                let left = Rect::from_min_size(avail.min, Vec2::new(half, avail.height()));
                let right = Rect::from_min_size(Pos2::new(avail.min.x + half + 8.0, avail.min.y), left.size());
                image_panel(ui, left, self.orig_tex.as_ref(), "Original");
                image_panel(ui, right, self.pix_tex.as_ref(), "Pixel-Art");
            }
        }
        ui.allocate_rect(avail, egui::Sense::hover());
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages(ctx);

        // Drag & Drop
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.into_iter().find(|p| is_image(p)) {
            self.load(p, ctx);
        }

        // Tastenkürzel
        let (open, save) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::O),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
            )
        });
        if open {
            self.open_dialog(ctx);
        }
        if save {
            self.save_dialog(ctx);
        }

        // ---------- Oberer Balken ----------
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Bild öffnen…").on_hover_text(format!("{MOD_KEY}+O")).clicked() {
                    self.open_dialog(ctx);
                }
                let can_save = self.result.is_some() && !self.saving;
                let hint = match self.prefs.target_dir(self.path.as_deref()) {
                    Some(d) if !self.prefs.ask => format!("{MOD_KEY}+S – speichert direkt nach:\n{}", d.display()),
                    Some(d) => format!("{MOD_KEY}+S – Dialog startet in:\n{}", d.display()),
                    None => format!("{MOD_KEY}+S"),
                };
                if ui.add_enabled(can_save, egui::Button::new("Speichern…")).on_hover_text(hint).clicked() {
                    self.save_dialog(ctx);
                }
                if ui
                    .add_enabled(self.batch.is_none(), egui::Button::new("Ordner verarbeiten…"))
                    .on_hover_text("Alle Bilder eines Ordners mit den aktuellen Einstellungen umwandeln")
                    .clicked()
                {
                    self.batch_dialog(ctx);
                }
                ui.separator();
                ui.selectable_value(&mut self.view, View::Split, "Vergleich");
                ui.selectable_value(&mut self.view, View::Pixel, "Pixel-Art");
                ui.selectable_value(&mut self.view, View::Original, "Original");
                if let Some(p) = &self.path {
                    ui.separator();
                    ui.weak(file_name(p));
                }
            });
            ui.add_space(4.0);
        });

        // ---------- Statusleiste ----------
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.rendering || self.loading || self.saving {
                    ui.spinner();
                }
                if let Some((done, total, cur)) = &self.batch {
                    ui.add(
                        egui::ProgressBar::new(*done as f32 / *total as f32)
                            .desired_width(220.0)
                            .text(format!("{done}/{total} {cur}")),
                    );
                }
                ui.label(&self.status);
            });
        });

        // ---------- Einstellungen ----------
        egui::SidePanel::left("settings").resizable(true).default_width(290.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(6.0);
                self.settings_ui(ui);
                ui.add_space(10.0);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| self.central_ui(ui));

        // ---------- Speicher-Einstellungen dauerhaft sichern ----------
        if self.prefs != self.saved_prefs {
            self.prefs.save();
            self.saved_prefs = self.prefs.clone();
        }

        // ---------- Neu rendern bei Änderungen ----------
        if self.params != self.last_params {
            self.last_params = self.params.clone();
            self.dirty_since = Some(Instant::now());
        }
        if let Some(t) = self.dirty_since {
            if !self.rendering && t.elapsed() >= DEBOUNCE {
                self.start_render(ctx);
            } else {
                ctx.request_repaint_after(DEBOUNCE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_speichern_laden_und_zielordner() {
        let tmp = std::env::temp_dir().join(format!("pxl_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("APPDATA", &tmp);

        assert_eq!(Prefs::load(), Prefs::default());
        let out = tmp.join("Mein Ausgabe Ordner");
        let p = Prefs { loc: SaveLoc::Folder, dir: Some(out.clone()), ask: false, scale: 4, small: true };
        p.save();
        assert_eq!(Prefs::load(), p);
        assert_eq!(p.target_dir(Some(Path::new("C:/x/bild.png"))), Some(out.clone()));

        let n = Prefs { loc: SaveLoc::NextToSource, ..p.clone() };
        assert_eq!(n.target_dir(Some(Path::new("C:/x/bild.png"))), Some(PathBuf::from("C:/x")));

        std::fs::create_dir_all(&out).unwrap();
        assert_eq!(unique_path(&out, "a_pxl"), out.join("a_pxl.png"));
        std::fs::write(out.join("a_pxl.png"), b"x").unwrap();
        assert_eq!(unique_path(&out, "a_pxl"), out.join("a_pxl_2.png"));

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

// ------------------------------------------------------------------ Helfer

fn window_icon() -> egui::IconData {
    let img = image_to_pxl::icon::icon_image(4); // 64×64
    egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
}

fn to_color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Zeichnet ein Bild eingepasst in `area`, mit Schachbrett für Transparenz.
fn image_panel(ui: &mut egui::Ui, area: Rect, tex: Option<&TextureHandle>, title: &str) {
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 6.0, Color32::from_gray(24));
    let label_h = 22.0;
    painter.text(
        area.min + Vec2::new(10.0, 4.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(14.0),
        Color32::from_gray(170),
    );
    let inner = Rect::from_min_max(area.min + Vec2::new(8.0, label_h + 4.0), area.max - Vec2::splat(8.0));
    let Some(tex) = tex else {
        painter.text(inner.center(), egui::Align2::CENTER_CENTER, "wird berechnet …", egui::FontId::proportional(16.0), Color32::from_gray(140));
        return;
    };
    let size = tex.size_vec2();
    let f = (inner.width() / size.x).min(inner.height() / size.y);
    // Bei Pixel-Art auf ganzzahlige Vergrößerung runden, damit alle Pixel gleich groß sind
    let f = if f >= 1.0 && size.x <= 1024.0 { f.floor() } else { f };
    let rect = Rect::from_center_size(inner.center(), size * f);

    // Schachbrett
    let cell = 12.0;
    let (c1, c2) = (Color32::from_gray(58), Color32::from_gray(44));
    let mut y = rect.min.y;
    let mut row = 0;
    while y < rect.max.y {
        let mut x = rect.min.x;
        let mut col = row;
        while x < rect.max.x {
            let r = Rect::from_min_max(Pos2::new(x, y), Pos2::new((x + cell).min(rect.max.x), (y + cell).min(rect.max.y)));
            painter.rect_filled(r, 0.0, if col % 2 == 0 { c1 } else { c2 });
            x += cell;
            col += 1;
        }
        y += cell;
        row += 1;
    }
    painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
}
