# AnsNew PDF Reader

A fast native PDF reader for people who actually work in a document: read, mark up, fill forms, export, and look up words — including Bangla — without opening a browser or a heavy office suite.

![License](https://img.shields.io/github/license/needyamin/pdf-reader)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)
![Rust](https://img.shields.io/badge/rust-1.98.1-orange)
![Version](https://img.shields.io/badge/version-5.0.1-green)

Built in **Rust**. Drawn with **egui** and **wgpu**. Pages come from **PDFium**, the same engine Chrome uses, on a tile renderer made for large files.

---

## Why this reader

Most “light” readers only scroll. Most “full” readers are slow, paid, or both. AnsNew sits in the middle:

- It stays quick on long documents because it draws only the tiles on screen.
- Highlights, notes, and form values are real PDF objects. Save them and they open in any other viewer.
- Click a word and get pronunciation, an offline English–Bangla meaning, and optional speech — no account, no API key.
- One window handles export, convert, and merge. Print uses the system dialog.

It is a ground-up rewrite of an earlier Electron app. The browser shell is gone. What remains is a small native program.

## What makes it different

| | AnsNew |
| --- | --- |
| Feel | Native window. Menus and toolbars stay the same size while the page zooms from 10% to 3200%. |
| Large files | Tile cache on a √2 zoom ladder. Fast scrolling cancels tiles you have already left. |
| Markup | Highlight, underline, strikeout, squiggly, rectangle, sticky note, and text box. Undo and redo. |
| Forms | Text fields and checkboxes you can fill on the page or in a sidebar, then save. |
| Learning | Offline dictionary, IPA, system text-to-speech, and a Google Translate link for the exact selection. |
| Scripts | Bundled fonts for Latin, Bengali, Devanagari, Arabic, Hebrew, Tamil, and Thai. |
| Ship it | Portable zip, two classic installers, and a signed MSIX. Double-click a PDF after install. |

Radio buttons, dropdowns, and list boxes are listed and explained in the Forms panel. The PDF binding this app uses cannot write those field types, so they are marked “not fillable” instead of failing on click. Text and checkboxes round-trip through Save.

## How fast it stays

The page is a grid of tiles, not one giant bitmap.

- Only tiles inside the viewport are rasterized. Zoom and scroll reuse bitmaps already in memory.
- Tiles render at the screen’s pixel density, so HiDPI stays sharp.
- PDFium runs on one background thread. The window never waits on it.
- A generation counter drops work for a view you have already scrolled past, so a fast flick does not build a queue.
- When the page is still, the app stops requesting frames.
- Export and merge run on that same engine thread, with progress and Cancel. A job that would render more than 100 megapixels is refused up front.

That is the speed model: draw what you see, throw away what you left, and keep the interface free.

## Tools

**Read.** Continuous or single page. Fit width (default), fit page, or 10%–3200%. Rotate. Thumbnails, outline, jump to page, full-document Find. Several files in tabs. Drag and drop, or pass a path on the command line. Encrypted files get a password prompt. Seven themes. Settings persist.

**Mark up.** Select, Highlight, Underline, Strikeout, Squiggly, Rectangle, Note, Type. A Comments list jumps to each note, edits its text, or deletes it. Undo (`Ctrl+Z`) and redo (`Ctrl+Y`). Flatten bakes notes and field values into the page.

**Fill.** Every open PDF is scanned for AcroForm fields. The Forms sidebar shows name, type, value, and how many are left. Type in the field on the page or in the list. Pick the font and size for typed values, including Bengali and the other bundled scripts. Save writes them into the PDF.

**Look up.** With the Select tool, click a word or drag a phrase. A card inside the window shows IPA (CMU dictionary), English–Bangla senses, and a Speak button (Windows, macOS, or Linux system voice). Bangla selections look up the English headword. A link opens Google Translate on the exact text. Dictionaries load in the background (~12 MB) so startup stays light. Turn any of this off in Settings.

**Export and print.** File → Export (`Ctrl+E`): current page to PNG or JPEG, every page to images, chosen pages to a new PDF, images to a PDF, or merge several PDFs. Print (`Ctrl+P`) hands a temporary PDF to the system print dialog.

**Files.** Save and Save As write to a temp file, then rename, so a crash cannot leave a half-written PDF. Closing a dirty tab asks first.

## Stack

| Piece | Role |
| --- | --- |
| Rust 1.98.1 (edition 2024) | Language. Release builds use thin LTO and a stripped binary. |
| egui / eframe | Immediate-mode interface: menus, sidebars, settings, export window. |
| wgpu | GPU backend behind the window. |
| PDFium (`pdfium-render`) | Open, rasterize, annotate, fill, save. The only native PDF seam. |
| `image` | PNG and JPEG for export and “images to PDF”. |

The code is a Cargo workspace: `core` (state), `pdf` (engine), `render` (tiles and hit testing), `ui` (widgets), `app` (window), `search`, `lens` (dictionary), `platform` (speech and URLs), and `xtask` (fetch, run, package). The UI sends a command, a pure reducer returns effects, and the shell does the I/O. Most of the logic is testable without a window.

## Comparison

A fair snapshot of where this app sits. “Fast” here means time-to-first-page and scroll on a long PDF, not a lab benchmark.

| | AnsNew | Browser viewer | SumatraPDF | Acrobat |
| --- | --- | --- | --- | --- |
| Engine | Native Rust + PDFium | The browser’s PDF plugin | Native, very small | Full commercial suite |
| Long documents | Tile cache, UI stays responsive | Fine until the tab gets heavy | Excellent | Capable, heavier |
| Highlights and notes | Saved into the PDF | Limited or none | Mostly view | Full |
| Fillable forms | Text and checkbox, saved | Basic | View | Full, including choice fields |
| Offline word lookup + speech | Built in (English–Bangla) | No | No | No |
| Install size | Small native app + PDFium | Already installed | Tiny | Large |
| Price | Free, MIT | Free | Free | Paid |
| Best when | You read, mark, fill, and study | You only need a quick look | You want the smallest viewer | You live in a full PDF suite |

## Build and run

You need Rust 1.98.1 (pinned in `rust-toolchain.toml`) and a network connection the first time, so the build can fetch PDFium.

```bash
cargo xtask fetch-pdfium     # once
cargo run -p pdfreader-app
```

Or both in one step:

```bash
cargo xtask run
```

Open a file, inspect one without a window, or run tests:

```bash
cargo run -p pdfreader-app -- path/to/document.pdf
cargo xtask inspect path/to/document.pdf
python tools/generate_fixtures.py target/test-pdfs
cargo test
```

`forms.pdf` from the fixture script is the quickest way to try the Forms sidebar.

### Shortcuts

| Keys | Action |
| --- | --- |
| `Ctrl+O` | Open |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+W` | Close tab |
| `Ctrl+F` | Find |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl +` / `Ctrl -` / `Ctrl 0` | Zoom in / out / 100% |
| `Ctrl+E` / `Ctrl+P` | Export / print |
| `Ctrl+B` | Sidebar |
| `F11` | Full screen |
| `Esc` | Leave full screen, or back to the Select tool |

Menus follow the usual desktop order: **File, Edit, View, Go, Settings, Help**. Settings opens a preferences window (appearance, reading, language tools, form text, performance). Help holds the shortcut list and About.

## Package for Windows

These tasks build a release binary, stage `pdf-reader.exe`, `pdfium.dll`, the licence texts, the three dictionaries, and `fonts/`, then wrap that folder. They run on Windows.

```bash
cargo xtask dist:portable   # zip — unpack and run
cargo xtask dist:nsis       # setup.exe  (needs NSIS)
cargo xtask dist:inno       # setup.exe  (needs Inno Setup)
cargo xtask dist:msix       # signed .msix
cargo xtask dist:msix -- --no-sign
```

Outputs land in `target/dist/`, named with the version from the root `Cargo.toml` (currently 3.0.0).

| Package | What you get |
| --- | --- |
| **Portable** | One folder in a zip. Settings still go to `%APPDATA%\pdf-reader`. |
| **NSIS / Inno** | Installs under Program Files, Start Menu shortcut, uninstall entry. Shows up as **AnsNew PDF Reader** in Open with and in Default apps. |
| **MSIX** | Full-trust desktop package with a `.pdf` file association. Signed with a self-signed certificate carrying the Store publisher identity. Trust that cert once — `certutil -user -addstore Root target\dist\pdf-reader-sign.cer` — then `Add-AppxPackage`. |

Installer tools, if you do not have them:

```bash
winget install -e --id NSIS.NSIS
winget install -e --id JRSoftware.InnoSetup
```

Release builds are windowed: no console on launch, and speech does not flash a terminal. The app runs on macOS and Linux too; the `dist:*` tasks are the Windows ship path. On other systems, `cargo xtask run` is the way to build and start it.

Windows will not silently replace a PDF app the user already chose. After install, pick AnsNew once under Open with → Always, or in Settings → Default apps.

## Privacy

The app collects nothing: no account, no analytics, no telemetry, and no network
code at all. Documents are opened, edited and saved on your own machine. The one
exception is the **Translate** button in the word-lookup popup, which opens a
Google Translate link containing the word you selected in your own browser.

The full policy is in [privacy_policy.html](privacy_policy.html) — it is the
document submitted to the Microsoft Store listing.

## License

**MIT** — see [LICENSE](LICENSE). Free to use, modify, and ship, including commercially, as long as the copyright and permission notice stay with the code.

PDFium is BSD-3-Clause. egui, eframe, wgpu, pdfium-render, and `image` are MIT or Apache-2.0.

## Author

**YAMiN HOSSAIN** — [@needyamin](https://github.com/needyamin)
