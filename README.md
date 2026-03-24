# Advanced PDF Reader

A professional, feature-rich PDF reader built with **Electron** and **TypeScript**, designed for seamless PDF viewing, annotation, form filling, and document management.

![License](https://img.shields.io/github/license/needyamin/pdf-reader)
![Platform](https://img.shields.io/badge/platform-Windows-blue)
![Electron](https://img.shields.io/badge/electron-34-brightgreen)

## Features

### Core Viewing
- High-quality PDF rendering powered by PDF.js
- Smooth scrolling with lazy page loading (renders only visible pages)
- Zoom in/out with keyboard shortcuts and GUI controls
- Page rotation, fullscreen mode, and continuous/single page view
- Bookmarks and thumbnail sidebar

### Annotation Tools
- **Highlight**, **Underline**, **Strikeout** text
- **Free Draw** with adjustable color palette
- **Sticky Notes** for comments
- **Eraser** tool
- Annotations are saved into the PDF via pdf-lib

### Form Filling
- Auto-detects PDF form fields (text, checkbox, dropdown)
- Inline interactive inputs positioned directly on the page
- Two-way sync between inline fields and sidebar panel
- Form data is embedded when saving/printing

### File Operations
- **Save** — overwrite current PDF with annotations & form data
- **Save As** — export current page as PDF, PNG, or JPEG
- **Print** — merges annotations, opens in system PDF viewer for printing

### Themes
- Dark, Light, Midnight, Rose, Forest, Sunset color themes
- Persistent theme selection via localStorage

### Auto Update
- Automatic update checks via GitHub Releases using `electron-updater`
- Prompts user to restart when a new version is downloaded

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+O | Open PDF |
| Ctrl+S | Save |
| Ctrl+Shift+S | Save As |
| Ctrl+P | Print |
| Ctrl+= | Zoom In |
| Ctrl+- | Zoom Out |
| Ctrl+0 | Reset Zoom |
| Ctrl+F | Find/Search |
| Ctrl+B | Toggle Sidebar |
| F11 | Fullscreen |

## Installation

```bash
git clone https://github.com/needyamin/pdf-reader.git
cd pdf-reader
npm install
```

## Development

```bash
npm run dev
```

## Build

Build a Windows installer (single `.exe`):

```bash
npm run dist
```

Output will be in the `out/` directory.

## Tech Stack

- **Electron** — Desktop application framework
- **TypeScript** — Main & preload processes
- **PDF.js** (`pdfjs-dist`) — PDF rendering
- **pdf-lib** — PDF modification, form filling, annotation embedding
- **electron-updater** — Auto-update from GitHub Releases
- **electron-builder** — Packaging & distribution

## Project Structure

```
PDF_READER/
├── src/
│   ├── main.ts          # Electron main process
│   └── preload.ts       # Context bridge API
├── renderer/
│   ├── index.html       # Application UI
│   ├── renderer.js      # Renderer logic
│   └── styles.css       # Themes & styling
├── assets/              # Icons & resources
├── package.json
└── tsconfig.json
```

## Auto-Update Setup

1. Bump `version` in `package.json`
2. Run `npm run dist`
3. Create a GitHub Release with the matching tag (e.g. `v1.1.0`)
4. Upload the `.exe` installer and `latest.yml` from `out/`
5. Existing users will be prompted to update automatically

## Author

**YAMiN HOSSAIN** — [@needyamin](https://github.com/needyamin)

## License

[MIT](LICENSE)
