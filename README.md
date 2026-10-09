# AnsNew PDF Reader

A fast, native, cross-platform PDF reader written in **Rust**, with an **egui** /

**wgpu** interface and a **PDFium** rendering engine.

This is a ground-up rewrite of an earlier Electron application. The Electron code

has been removed; what remains is a Rust Cargo workspace with a tile-based GPU

renderer built for large documents.

![License](https://img.shields.io/github/license/needyamin/pdf-reader)

![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)

![Rust](https://img.shields.io/badge/rust-1.98.1-orange)


## Features

**Viewing**

- Tile-based rendering: only the tiles on screen are rasterized, on a √2 zoom 

  ladder, so zoom and scroll reuse cached bitmaps.
- Continuous and single-page modes; 10%–3200% zoom, **fit width** (the default)

  and **fit page**.
- `Ctrl +` / `Ctrl -` / `Ctrl 0` zoom the **page, not the interface**: egui's

  whole-GUI zoom is switched off, so menus and toolbars keep their size however

  far the document is scaled.
- Rotate either way, crisp on HiDPI (tiles render at the device pixel ratio), and

  stale tiles are cancelled by a generation counter so fast scrolling never builds

  a backlog.

**Navigation**

- Page thumbnails sidebar (lazy, cached) and a document outline with click-to-jump.
- Previous/next page, jump to page, Page Up/Down, Home/End.
- Full-document text search with matching-page results and click-to-jump.
- **Find**: one `input-group` box — a magnifier and the word *Find* in a leading

  addon, the query, and a clear button that appears once there is a query. It is

  the same control in the toolbar and in the Search sidebar; **Enter** runs it.

**Annotations**

- Tool strip above the page: Select, Highlight, Underline, Strikeout, Squiggly,

  Rectangle, Note, Type.
- Drag to draw with a live rubber-band preview; the Note tool places a sticky note

  with a click, and Type places an editable text box.
- **Comments sidebar** listing every annotation with its kind, page and text. Click

  to jump to it, edit its text in place, or delete it (button, or shift-click the

  row).
- Real PDF annotations, so they survive Save and are visible in every other viewer.

  Flattening bakes them into the page.
- **Undo** (`Ctrl+Z`) and **Redo** (`Ctrl+Shift+Z`, or `Ctrl+Y`) step through

  creates, deletes and text edits one at a time. A new edit clears the redo

  history, so redo can only ever re-apply what was just undone.

**Form fields**

- Every opened PDF is scanned for AcroForm fields.
- **Forms sidebar** listing each field with its name, type, current value and

  read-only state, plus an `N fields · M to fill` progress summary. Click a row to

  jump to the field, or a field on the page to select it; the two stay in sync

  through the reducer.
- Filling: text fields are editable both in the sidebar and in an editor overlaid

  on the page at the field's real rectangle, committing on Enter or blur rather

  than per keystroke. Checkboxes toggle by clicking. Radio buttons and dropdowns

  are not offered — see the note below.
- **Checkbox and radio state is drawn by the app**, so a checked box shows a tick

  (and a radio a dot) the instant it is clicked. `PDFium` rasterizes a widget's

  original appearance but does not regenerate it after a value is written, so

  relying on the page render alone left a checked box looking empty.
- **Radio buttons and dropdowns are shown as "not fillable", not offered.** There is

  no write path for them in the PDF binding: a radio group keeps its value on the

  parent field dictionary (verified against the raw PDFium API, where

  `FPDFAnnot_GetLinkedAnnot(annot, "Parent")` returns null — the same wall Chrome

  hits), and the combo and list box wrappers have no setter at all. Rather than let

  a click fail, the app marks those fields and explains why at the top of the Forms

  panel. Text fields and checkboxes are unaffected and round-trip through a save.
- **XFA forms** (whose AcroForm widgets are only a compatibility shell) are called

  out in that same note, so the limitation is known before filling.
- Field names in the Forms list keep the **last two path segments**: XFA names are

  long shared prefixes (`topmostSubform[0].Page1[0].…`) and truncating the end made

  every row read the same.
- Whitespace counts as blank, so a form that ships a combo box pre-filled with

  spaces no longer reports as partly complete.
- **Form text font and size** (Settings → **Text**, and the same controls at the top

  of the Forms sidebar): pick the face used for typed values — Bengali, Arabic,

  Devanagari, Hebrew, Tamil, Thai or the default Latin — and a size, or leave it on

  **Auto** to fit each field's height. This is what makes filling a form in Bengali

  legible instead of boxes.
- Values are written into the open document via PDFium, so **Save** (or "Save and

  close") persists them for every other viewer.
- Editable fields show a discoverable outline and the selected field a highlight

  ring, drawn through the screen↔page transform so they land correctly at any zoom

  and rotation.
- Text, checkbox, radio group, combo box, list box and push button widgets are all

  recognised and reported.

**Files**

- Open via dialog (`Ctrl+O`), drag and drop, or a path on the command line.
- Multiple documents open at once, in tabs (`Ctrl+W` to close).
- Encrypted PDFs open through a non-blocking password prompt with retry feedback.
- **Save** (`Ctrl+S`) and **Save as** (`Ctrl+Shift+S`) write the document back

  atomically: bytes go to a temp file first and are renamed over the destination,

  so a crash can never leave a half-written PDF behind.
- **Flatten** bakes field values and annotations into page content, so the result

  is guaranteed to look the same in every viewer.
- Closing a tab with unsaved changes prompts first — Save and close, Discard, or

  Cancel.

**Export, print and compose**

Everything that turns a document into files lives in one window, reached from

**File → Export** (or `Ctrl+E`). Picking a job from the menu opens the window

already set to that job, and the job can be changed from inside it:

- **Page to image** — the page on screen, as PNG or JPEG.
- **Pages to images** — one file per page, named `<document>-0001.png` and so on.
- **Pages to PDF** — a new document containing the pages you name, not just the one

  on screen.
- **Images to PDF** — any number of PNG/JPEG files, one page each.
- **Merge PDFs** — several documents combined into one, in the order listed.

The window keeps one shape, so switching jobs does not make it jump around: a task

strip down the left, per-job settings (format, a 72/150/300/600 dpi preset beside a

live pixel-size readout, first/last page fields, or an A4/Letter sheet size), a

reorderable file list for the jobs that read files, and a live page preview taken

from the same thumbnail cache the sidebar already draws from. The \*\*Export button

stays disabled until the job is ready\*\*, and the line beside it is the same check

that gates the button, so it always names the next step instead of failing after the

fact.

**Print** (`Ctrl+P`) hands a temporary PDF to the system print handler, for the

whole document or the current page. There is no silent printing: the shell always

shows its own printer dialog.

Long jobs run on the engine thread and report progress in the status bar with a

Cancel button. Every export refuses to start if it would have to render more than

100 megapixels, which is what stops an accidental 3200% zoom from allocating

gigabytes before failing.

**Appearance**

- Six themes: Dark, Light, Midnight, Rose, Forest, Sunset.
- Theme and sidebar visibility persist between runs.

**Word lookup (offline dictionary)**

- Built-in reading aid that brings LingoLearn-style quick lookups into the reader

  itself — no separate app, no network, no API keys. It works on any PDF whose text

  layer is selectable.
- **Click a word** to look it up, or **drag to select a phrase or paragraph**: the

  selection opens a floating card anchored next to the clicked word, inside the PDF

  window (never a separate window).
- The card shows:
  - **Phonetics (IPA)** from the bundled CMU Pronouncing Dictionary, so you can see

    how the word is said even when the dictionary has no Bangla gloss.
  - **Dictionary meaning** from the bundled E2B (English→Bangla) dictionary, with

    part-of-speech tags and several senses grouped and trimmed for quick scanning.
  - A **Google Translate** link for the exact selected text, opened in your browser

    with the chosen target language.
- **Phrases**: a whole dragged selection is translated word-by-word. Every word that

  has a Bangla gloss contributes a sense; words with no entry are listed once at the

  bottom so you know what was missed (rather than failing the whole lookup).
- **Bangla→English**: if you select Bangla text, the card looks it up in reverse

  against the E2B dictionary and shows the English headword.
- **Language switch**: a dropdown in the card (and the persisted `lens_target`

  setting) changes only the Google Translate target — `bn`, `en`, `zh-CN`, and any

  other `translate.google.com` language code work. The offline dictionaries are

  English→Bangla and do not change.
- **System TTS**: a **Speak** button reads the selected text aloud using the

  operating-system speech engine (Windows SAPI, macOS `say`, Linux `spd-say` /

  `espeak`) — no bundled voices, no network. Turn on **Speak automatically on

  selection** (Settings → Language tools) and the word or phrase is read aloud the

  moment the card opens, with no second click.
- **Settings window**: the **Settings** item in the menu bar opens a full

  preferences window. Turn **Word lookup**, **Speak (text to speech)** and the

  **Google Translate link** on or off, pick the translate target language, and

  more. Every choice is saved to `settings.json` and applies immediately; switching

  word lookup off drops any open card and ignores new selections.
- **Scope**: lookups fire only with the **Select** tool. The annotation tools

  (Highlight, Underline, …) keep their own draw behaviour, so selecting text never

  collides with drawing a highlight.
- **Lazy, background load**: the three dictionaries (~12 MB total: `E2Bdatabase.json`,

  `bangla_dictionary.txt`, `cmudict-0.7b-ipa.txt`) are parsed on a background thread

  while the app launches, so startup stays fast; the card shows "Loading…" until they

  are ready. The files live in `assets/lens/` and are resolved next to the executable.
- Press **Esc** (or click the ×) to dismiss the card.

**Settings window**

Click **Settings** in the menu bar to open the preferences window directly. It
has a fixed category rail on the left, the options for the selected category in
a scrolling centre, and an action bar along the bottom (**Reset to defaults** /
**Done**). Every change is written to `settings.json` and takes effect
immediately:

| Category        | Options                                                                                       |
| --------------- | --------------------------------------------------------------------------------------------- |
| **Appearance**  | Theme (7 themes), interface scale (0.75×–2.00×), show the sidebar on startup                    |
| **Reading**     | Default view mode (continuous / single page), default zoom (fit width / fit page / 100%), open maximized |
| **Language**    | Word lookup, Speak (TTS), speak automatically on selection, Google Translate link, translate target language (12 languages) |
| **Text**        | Form-text font family (Default / Noto Sans / Bengali / Devanagari / Arabic / Hebrew / Tamil / Thai) and size, with a live preview |
| **Performance** | Preload nearby pages (tile prefetch), performance overlay (FPS, cached tiles, zoom)             |

The window **opens maximized by default**: it fills the screen *with* its title
bar, so the minimize / maximize / close buttons stay where you expect them. The
preference lives under Reading → Window. **F11** (or Escape) toggles true
borderless full screen, which hides the title bar. Help → About opens a separate
**About Us** window rather than a settings page.

Reading defaults apply to every document you open, and to the current one
straight away.

## Architecture

A Cargo workspace of focused crates:

| Crate      | Responsibility                                                                                                                                            |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `core`     | Domain model, `Command`/`Effect` types, the pure reducer and app state; owns the form-field and rectangle types every layer shares                        |
| `pdf`      | The `PdfEngine` trait and its PDFium backend (the only FFI seam)                                                                                          |
| `render`   | Zoom ladder, tile keys, viewport math, atlas, scheduling, and the screen↔page hit-testing transform (`render::hit`)                                       |
| `ui`       | egui widgets and theming (no application state)                                                                                                           |
| `app`      | Window, panels, the tile canvas, the engine thread and the export window                                                                                  |
| `search`   | Pure text matching primitives used by the asynchronous search flow                                                                                        |
| `lens`     | Offline word lookup: parses the bundled E2B/Bangla/IPA dictionaries and answers a selection with a meaning, IPA, and a Google Translate link — no network |
| `platform` | OS adapters with no extra crates: system speech (`Speaker`, SAPI/`say`/`spd-say`) and opening URLs in the browser                                         |
| `store`    | Persistence reserved for later phases                                                                                                                     |
| `xtask`    | Build automation: PDFium fetch, build, run, inspect, and Windows packaging (`dist:*`)                                                                     |

State flows one way. The UI emits a `Command`, the pure reducer turns it into

`Effect`s, and the shell executes them and feeds results back as further commands.

Keeping the reducer free of I/O and transient widget state is why most of the

interesting logic is testable without a window.

### Why a single engine thread

PDFium is not thread-safe. `pdfium-render` makes it *safe* by wrapping every call in

a process-global mutex, which serialises them. The architecture therefore uses

exactly one thread that owns PDFium, and the UI never blocks on it: tile requests and

results travel over channels, and a generation counter drops work for viewports the

user has already left.

## Tech stack

- **Rust** — edition 2024, toolchain pinned to 1.98.1
- **egui** / **eframe** / **wgpu** — immediate-mode UI and GPU rendering
- **PDFium** (via `pdfium-render`) — PDF parsing and rasterization
- **crossbeam-channel** — engine/UI messaging
- **image** — PNG/JPEG encode and decode for the export jobs

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

To print timing and structure for a PDF without opening a window:

```bash
cargo xtask inspect path/to/document.pdf
```

### Menu bar

The menu bar follows the layout desktop software uses — **File**, **Edit**,
**View**, **Go**, **Settings**, **Help**:

| Menu         | Contents                                                                                            |
| ------------ | --------------------------------------------------------------------------------------------------- |
| **File**     | Open…, Save, Save As…, Flatten Document, Export ▸, Print…, Print Current Page, Close Tab, Exit         |
| **Edit**     | Undo, Redo, Find…                                                                                     |
| **View**     | Zoom In/Out, Actual Size, Fit Width, Fit Page, Page Display ▸ (Single Page / Continuous), Rotate View ▸ (Clockwise / Counterclockwise), Show/Hide Sidebar, Full Screen, Theme ▸ |
| **Go**       | First Page, Previous Page, Next Page, Last Page                                                       |
| **Settings** | Opens the preferences window (a direct action, not a dropdown)                                        |
| **Help**     | Keyboard Shortcuts, About AnsNew PDF Reader                                                           |

Help → **Keyboard Shortcuts** opens a shortcut reference; Help → **About AnsNew
PDF Reader** opens the separate **About Us** window (photo, role, profile links).

### Keyboard shortcuts

| Keys                                 | Action                           |
| ------------------------------------ | -------------------------------- |
| `Ctrl+O`                             | Open a document                  |
| `Ctrl+S` / `Ctrl+Shift+S`            | Save / Save as                   |
| `Ctrl+W`                             | Close the current tab            |
| `Ctrl+P`                             | Print                            |
| `Ctrl+E`                             | Export, convert or merge         |
| `Ctrl+Z` / `Ctrl+Shift+Z` / `Ctrl+Y` | Undo / Redo / Redo               |
| `Ctrl+F`                             | Find (focus the search box)      |
| `Ctrl +` / `Ctrl -` / `Ctrl 0`       | Zoom in / out / actual size      |
| `Page Up` / `Page Down`              | Previous / next page             |
| `Space` / `Shift+Space`              | Next / previous page             |
| `Home` / `End`                       | First / last page                |
| `Ctrl+B`                             | Show or hide the sidebar         |
| `F11`                                | Fullscreen                       |
| `Esc`                                | Leave fullscreen, or Select tool |

Fit width and fit page live in the **View** menu; fit width is the default.

### Application icon

`assets/icon.png` is embedded into the binary and handed to the window at startup,

so the title bar, taskbar and Alt-Tab entries are branded even when the executable is

moved away from `assets/`. On Windows, `assets/icon.ico` is additionally compiled

into the executable as a resource by `crates/app/build.rs`, which is what makes

Explorer show the icon for the file itself. Both happen automatically on build and

rebuild when either asset changes. To confirm the resource made it into a built

binary:

```bash
python tools/check_exe_icon.py target/debug/pdf-reader.exe
```

### Try the form features

```bash
python tools/generate_fixtures.py target/test-pdfs          # creates forms.pdf
cargo run -p pdfreader-app -- target/test-pdfs/forms.pdf
```

`forms.pdf` is a one-page AcroForm with a text field, checkbox, radio group,

dropdown, list box and push button. The **Forms** tab in the left sidebar lists them;

click a field on the page or in the list to edit it, then `Ctrl+S` to save and reopen

the file anywhere to see the values persisted.

### Tests

```bash
python tools/generate_fixtures.py target/test-pdfs   # deterministic fixtures (once)
cargo test
```

The fixture generator is dependency-free and writes plain uncompressed PDFs,

including `forms.pdf` — one widget of every common type, each with a real appearance

stream. Tests skip cleanly when the fixtures (or the PDFium library) are absent.

`crates/pdf/tests/forms.rs` verifies field types, values, radio groups, combo options

and widget rectangles against that fixture. `crates/pdf/tests/export.rs` covers the

output side the same way: it merges a document with itself and checks the page count

doubles, extracts a single page and checks it is a one-page document, writes a page

image and decodes it back, turns images into a PDF and checks the page count, lays

images onto standard sheets, and cancels a long job mid-flight to prove it stops

before the next item rather than after the last.

### Packaging (Windows)

Four `xtask` tasks turn a release build into shippable Windows artifacts. Each one

stages PDFium, builds the release binary, and assembles the ship set —

`pdf-reader.exe`, `pdfium.dll`, `LICENSE`, PDFium's third-party licence texts, the

three word-lookup dictionaries (`E2Bdatabase.json`, `bangla_dictionary.txt`,

`cmudict-0.7b-ipa.txt`) and the bundled `fonts/` — into `target/dist/stage`, then

packages it:

The dictionaries and the bundled Noto fonts (Latin, Bengali, Devanagari, Arabic,

Hebrew, Tamil, Thai, plus Noto Sans Symbols for arrows like `→`) sit directly beside

the executable, so the offline lookup, IPA, Google Translate link and non-Latin scripts

all work in the packaged build just as they do in a debug run (the loaders look next to

the exe first, then fall back to `assets/lens` / `assets/fonts` for a `cargo run` from

the repository root).

```bash
cargo xtask dist:portable   # target/dist/pdf-reader-<version>-portable-x64.zip
cargo xtask dist:nsis       # target/dist/pdf-reader-<version>-setup.exe
cargo xtask dist:inno       # target/dist/pdf-reader-<version>-inno-setup.exe
cargo xtask dist:msix       # target/dist/pdf-reader-<version>-x64.msix (signed)
cargo xtask dist:msix -- --no-sign
```

- **Portable** — a zip with one top-level folder; unpack anywhere and run. Settings

  still live in `%APPDATA%\pdf-reader`.
- **NSIS** (`makensis`) and **Inno Setup** (`ISCC`) — installers for `Program Files`

  with a Start Menu shortcut, an Add/Remove Programs entry and an uninstaller. They

  register the app as a first-class PDF handler so it appears as **"AnsNew PDF Reader"** in

  right-click → **Open with** (via `FriendlyAppName`), in the **Open with** submenu

  (via `OpenWithProgids`) and in **Settings → Default apps** (via `RegisteredApplications`

  + `Capabilities`). A best-effort per-user default is written too, so a double-clicked

  PDF opens this reader unless another viewer is already pinned (Windows keeps an

  explicit `UserChoice` ahead of it; the user can always change it in Default apps).

  Both tools install with `winget install -e --id NSIS.NSIS` and

  `winget install -e --id JRSoftware.InnoSetup`.
- **MSIX** — a full-trust desktop package built with the Windows SDK's `makeappx` and

  signed with a self-signed certificate (`CN=YAMiN HOSSAIN`) that xtask creates in the

  current-user store and reuses. The manifest declares a `windows.fileTypeAssociation`

  for `.pdf`, so after install the app appears as **"AnsNew PDF Reader"** in Open with and in

  Default apps. Because it is self-signed, a machine must trust the certificate before

  installing — the task prints the exact `certutil` command; side-load with

  `Add-AppxPackage`.

The version in every artifact name comes from `[workspace.package] version` in the root

`Cargo.toml`. The installer definitions are generated into `target/dist` at run time,

so they cannot drift from the manifest; to change the packaging itself, edit

`xtask/src/dist.rs`.

Release builds are windowed (`windows_subsystem`), so launching from Explorer or a

shortcut shows no console window; debug builds keep the console for tracing output
(`RUST_LOG` overrides the level). `wgpu_hal`'s warnings are dropped by default:
without the Vulkan SDK installed its loader logs a "registry lookup failed to get
layer manifest files" warning on every launch, which is harmless noise about the
optional SDK layers and not about this app.

The helper processes the app starts are windowless too: the speech engine's

PowerShell is spawned with `CREATE_NO_WINDOW` (a GUI process spawning a console

program otherwise flashes a terminal on every utterance), and URLs open through

`rundll32` rather than `cmd`. So neither the EXE nor the MSIX build ever shows a

stray terminal.

## License

**MIT** — see [LICENSE](LICENSE). Free to use, modify and distribute, including

commercially, provided the copyright notice and permission notice are kept.

Bundled third-party components keep their own licences:

| Component     | Licence           |
| ------------- | ----------------- |
| PDFium        | BSD-3-Clause      |
| egui / eframe | MIT or Apache-2.0 |
| wgpu          | MIT or Apache-2.0 |
| pdfium-render | MIT or Apache-2.0 |
| image         | MIT or Apache-2.0 |

## Author

**YAMiN HOSSAIN** — [@needyamin](https://github.com/needyamin)
