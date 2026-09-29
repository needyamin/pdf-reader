# PDF Reader

A fast, native, cross-platform PDF reader written in **Rust** with an
**egui**/**wgpu** interface and a **PDFium** rendering engine.

This is a ground-up rewrite of an earlier Electron application. The Electron
code has been removed; what remains is a Rust Cargo workspace with a tile-based
GPU renderer built for large documents.

![License](https://img.shields.io/github/license/needyamin/pdf-reader)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)

## Features

### Viewing
- Tile-based rendering: only the tiles on screen are rasterized, on a √2 zoom
  ladder, so zoom and scroll reuse cached bitmaps.
- Continuous and single-page modes.
- Zoom from 10% to 3200%, with **fit width** and **fit page**.
- Rotate clockwise / counter-clockwise.
- Crisp on HiDPI displays (tiles are rendered at the device pixel ratio).
- Smooth scrolling on large documents; stale tiles are cancelled by a
  generation counter so fast scrolling never builds a backlog.

### Navigation
- Page thumbnails sidebar (lazy, cached).
- Document outline sidebar with click-to-jump.
- Previous/next page, jump to page, Page Up/Down, Home/End.
- Full-document text search with matching-page results and click-to-jump.

### Form fields
- AcroForm detection: every opened PDF is scanned for interactive fields.
- **Forms sidebar** listing each field with its name, type, current value and
  read-only state, plus a `N fields · M to fill` progress summary.
- Click a row to jump to the field; click a field on the page to select it.
  The two stay in sync through the reducer.
- On-page overlay: editable fields show a discoverable outline, the selected
  field a highlight ring — drawn through the screen↔page transform, so they
  land correctly at any zoom and rotation.
- Text, checkbox, radio group, combo box, list box and push button widgets are
  all recognised and reported.

### Files
- Open via dialog (`Ctrl+O`), drag and drop, or a path on the command line
  (`pdf-reader file.pdf`).
- Multiple documents open at once, in tabs (`Ctrl+W` to close).
- Encrypted PDFs open through a non-blocking password prompt with retry feedback.

### Appearance
- Six themes: Dark, Light, Midnight, Rose, Forest, Sunset.
- Theme and sidebar visibility persist between runs.

## Not yet implemented

These are deliberately **not** exposed in the UI — there are no dead controls:

- Editing and saving field values (detection, listing and selection are done)
- Text selection and copy
- Annotations and the typewriter tool
- Save / Save As / export / print
- Recent-files menu

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+O` | Open PDF |
| `Ctrl+W` | Close tab |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+=` | Zoom in |
| `Ctrl+-` | Zoom out |
| `Ctrl+0` | Reset zoom |
| `F11` | Fullscreen |
| `Page Up` / `Page Down` | Previous / next page |
| `Home` / `End` | First / last page |

## Build and run

The build needs a PDFium shared library. Fetch the pinned build once, then run:

```bash
cargo xtask fetch-pdfium     # download + stage PDFium (once)
cargo run -p pdfreader-app   # build and launch
```

Or, in one step:

```bash
cargo xtask run
```

To open a file directly:

```bash
cargo run -p pdfreader-app -- path/to/document.pdf
```

### Inspect a PDF

```bash
cargo xtask inspect path/to/document.pdf
```

Prints page count, page geometry, encryption status and the outline tree using
the real engine.

### Tests

```bash
python tools/generate_fixtures.py target/test-pdfs   # deterministic fixtures (once)
cargo test
```

The fixture generator is dependency-free and writes plain uncompressed PDFs,
including `forms.pdf` — a one-page AcroForm with one widget of every common
type, each with a real appearance stream. The form-detection tests in
`crates/pdf/tests/forms.rs` verify types, values, radio groups, combo options
and widget rectangles against it. Tests skip cleanly when the fixtures (or the
PDFium library) are absent.

## Architecture

A Cargo workspace of focused crates:

| Crate | Responsibility |
|---|---|
| `core` | Domain model, `Command`/`Effect` types, the pure reducer and app state; owns the form-field and rectangle types every layer shares |
| `pdf` | The `PdfEngine` trait and its PDFium backend (the only FFI seam) |
| `render` | Zoom ladder, tile keys, viewport math, atlas, scheduling, and the screen↔page hit-testing transform (`render::hit`) |
| `ui` | egui widgets and theming (no application state) |
| `app` | Window, panels, the tile canvas and the engine thread |
| `platform`, `store` | Platform adapters and persistence reserved for later phases |
| `search` | Pure text matching primitives used by the asynchronous search flow |
| `xtask` | Build automation: PDFium fetch, build, run, inspect |

### Why a single engine thread

PDFium is not thread-safe. `pdfium-render` makes it *safe* by wrapping every
call in a process-global mutex, which serialises them. The architecture
therefore uses exactly one thread that owns PDFium, and the UI never blocks on
it: tile requests and results travel over channels, and a generation counter
drops work for viewports the user has already left.

## Tech stack

- **Rust** (edition 2024)
- **egui** / **eframe** / **wgpu** — immediate-mode UI and GPU rendering
- **PDFium** (via `pdfium-render`) — PDF parsing and rasterization
- **crossbeam-channel** — engine/UI messaging

## Author

**YAMiN HOSSAIN** — [@needyamin](https://github.com/needyamin)

## License

[MIT](LICENSE)
