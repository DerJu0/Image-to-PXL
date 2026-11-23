# Image to PXL

Rust-Programm mit GUI und Kommandozeile, das Bilder in Pixel-Art umwandelt (begrenzte Palette, flache Farbflächen, erhaltene Lineart, dunkle Außenkontur).

## Download

Fertige Windows-Programme gibt es unter **Releases** auf der rechten Seite dieser GitHub-Seite. `pxl-gui.exe` herunterladen und per Doppelklick starten, eine Installation ist nicht nötig.

## Bauen

Voraussetzung: [Rust](https://rustup.rs) (stable). Unter Windows zusätzlich die „Visual Studio Build Tools“ mit C++, die rustup bei der Installation anbietet.

```powershell
git clone <Repository-URL>
```
```powershell
cd image-to-pxl
```
```powershell
cargo build --release
```

Danach liegen unter `target\release\` zwei Programme:

- `pxl-gui.exe`: grafische Oberfläche (zum Doppelklicken)
- `pxl.exe`: Kommandozeile; per Doppelklick, also ohne Argumente, startet sie ebenfalls die GUI

Im Projektordner liegt außerdem eine Kopie der GUI als `Image to PXL.exe`. Nach einem neuen Build muss sie neu kopiert werden:

```powershell
Copy-Item target\release\pxl-gui.exe "Image to PXL.exe" -Force
```

## Lizenz

[MIT](LICENSE): Nutzen, Verändern und Weitergeben ist erlaubt, solange der Lizenzhinweis erhalten bleibt.

## Icon

Das App-Icon ist eine 16×16-Pixel-Art, definiert in [src/icon.rs](src/icon.rs). `build.rs` bettet `assets/icon.ico` (16–256 px) beim Bauen in beide Exe-Dateien ein; die GUI nutzt es zusätzlich als Fenster- und Taskleisten-Icon. Nach Änderungen am Motiv:

```powershell
cargo run --release --example make_icon
```

## GUI

```powershell
.\target\release\pxl-gui.exe
```

- Bild über **Bild öffnen…** (Strg+O), per Drag & Drop oder als Startargument laden
- Links alle Einstellungen (Auflösung inkl. Schnellwahl, Farben, Stil, Linienerkennung); die Vorschau rechnet bei jeder Änderung live neu
- Ansichten: **Vergleich** (Original | Pixel-Art), **Pixel-Art**, **Original**; Transparenz wird als Schachbrett angezeigt
- **Speichern…** (Strg+S) exportiert hochskaliert als PNG, optional zusätzlich die 1:1-Version
- **Ordner verarbeiten…** wandelt alle Bilder eines Ordners mit den aktuellen Einstellungen um

### Speicherort

Im Bereich **Speichern** links unten:

- **Neben dem Originalbild** oder **Eigener Ordner** (über „Ordner wählen…“, mit „Im Explorer öffnen“)
- **Vor dem Speichern nachfragen**
  - an: Der Dateidialog startet im eingestellten Ordner.
  - aus: „Speichern“ legt die Datei sofort dort ab. Vorhandene Dateien werden nicht überschrieben, sondern bekommen `_2`, `_3` … angehängt.
- Auch **Ordner verarbeiten…** nutzt diesen Speicherort.
- Die Einstellungen bleiben nach dem Neustart erhalten. Sie liegen in `%APPDATA%\Image to PXL\einstellungen.txt`.

In der Kommandozeile wird der Zielordner mit `-o` angegeben.

## Kommandozeile

```powershell
.\target\release\pxl.exe Testdaten -o Ausgabe
```

```powershell
.\target\release\pxl.exe Testdaten -o Ausgabe --crop --res x128 --colors 24 --small
```

### Auflösung einstellen

Die Auflösung der Pixel-Art wird **exakt** getroffen (gebrochene Blockgrößen), das Seitenverhältnis bleibt erhalten:

| Aufruf | Ergebnis |
|---|---|
| `--res x160` oder `-H 160` | 160 Pixel hoch, Breite automatisch (Standard) |
| `--res 128x` oder `-W 128` | 128 Pixel breit, Höhe automatisch |
| `--res 64x64` | passt in eine 64×64-Box |
| `--res 64x64 --pad` | exakt 64×64, Motiv zentriert, Rest Hintergrund |
| `-p 16` | feste Blockgröße: 16×16 Originalpixel = 1 Pixel |

Mit `--crop` bezieht sich die Auflösung auf den zugeschnittenen Bereich, also auf die Figur statt auf die leere Leinwand. Die Größe der gespeicherten Datei steuerst du mit `--scale` (Standard: ungefähr Originalgröße); mit `--small` wird zusätzlich die 1:1-Version gespeichert.

| Option | Standard | Bedeutung |
|---|---|---|
| `-R, --res` | – | Auflösung als `BxH`, `Bx` oder `xH` |
| `-W, --width` | – | Breite der Pixel-Art in Pixeln |
| `-H, --height` (`-r, --rows`) | 160 | Höhe der Pixel-Art in Pixeln |
| `--pad` | aus | Auf exakt `BxH` auffüllen (Motiv zentriert) |
| `-p, --pixel-size` | – | Feste Blockgröße in Originalpixeln (überschreibt die Auflösung) |
| `-c, --colors` | 24 | Palettengröße |
| `-s, --scale` | auto | Vergrößerung der Ausgabe (auto ≈ Originalgröße) |
| `--small` | aus | Zusätzlich 1:1-Version (`*_pxl_1x.png`) speichern |
| `--crop` | aus | Auf den Bildinhalt zuschneiden |
| `-t, --transparent` | aus | Hintergrund transparent (bei transparenten Quellen automatisch) |
| `--no-outline` | – | Keine dunkle Außenkontur |
| `--no-cleanup` | – | Einzelpixel/Rauschen nicht entfernen |
| `--no-bg` | – | Keine Hintergrunderkennung |
| `--saturation` | 1.15 | Sättigungsfaktor |
| `--contrast` | 1.05 | Kontrastfaktor |
| `--line-delta` | 0.14 | Empfindlichkeit der Linienerkennung (kleiner = mehr Linien) |
| `--line-threshold` | 0.30 | Linienanteil pro Block, ab dem er zur Kontur wird |

## Algorithmus (src/pixelate.rs)

1. Hintergrund: Alpha-Kanal bzw. Flood-Fill von einfarbigen Bildecken
2. Raster: Block pro Ausgabepixel; dominante Farbe per 2-Means in Oklab (harte Kanten statt Mischfarben)
3. Lineart: Pixel deutlich dunkler als die Umgebung werden bevorzugt, damit Konturen erhalten bleiben; isolierte „Linien“ (Faltenrauschen) werden verworfen
4. Palette: k-Means++ in Oklab, Farbigkeit doppelt gewichtet
5. Aufräumen isolierter Pixel, dann Selective Outline zum Hintergrund hin
6. Hochskalieren per Nearest Neighbor

## Beispiel
- Original
<img width="500" height="550" alt="d75f5341-70ce-4181-babe-dd826c26c6bc" src="https://github.com/user-attachments/assets/7dea20a3-5dc5-49e6-9e20-1614e3afb675" />

- Pixel Version
<img width="500" height="550" alt="d75f5341-70ce-4181-babe-dd826c26c6bc_pxl" src="https://github.com/user-attachments/assets/9559fc3d-9164-4b2a-b91e-d31a63255d11" />


