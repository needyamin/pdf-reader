# Advanced PDF Reader

A modern, professional PDF reader and annotator for Windows, built with Tkinter and PyMuPDF. Inspired by Adobe Photoshop, with a dark theme, responsive UI, annotation tools, and form fill support.

## Features
- Dark, modern UI (Photoshop-inspired)
- Sidebar with page thumbnails (active page highlighted with page number badges)
- Main PDF display with zoom, fit, and rotation
- **Keyboard scrolling**: Up/Down arrows for vertical scrolling, Ctrl+Left/Right for horizontal scrolling
- **Session restore**: Automatically remembers last opened PDF and page position
- **Default PDF association**: Can be set as default PDF reader for Windows
- Toolbar with annotation tools:
  - Highlight, Draw, Text Note, Image, Form Fill, Eraser
- Undo/Redo for all annotation actions (Ctrl+Z, Ctrl+Y)
- Keyboard shortcuts for all major actions
- Form fill: text, checkbox, radio, dropdown, list
- Windows auto-startup option
- Build to standalone `.exe` (no Python required)
- Professional Windows installer with Inno Setup

## Requirements
- Python 3.8+
- PyMuPDF
- Pillow
- pywin32
- pystray
- requests
- (Tkinter is included with standard Python on Windows)

## Installation & Build

1. **Clone this repo**
2. **Install Python 3.8+** (Windows recommended)
3. **Install PyInstaller** (for building .exe):
   ```sh
   pip install pyinstaller
   ```
4. **Build the app:**
   ```sh
   python build.py
   ```
   The standalone `.exe` will be in the `/public/` folder.
   The `/public` folder will contain the built `.exe`, an Inno Setup script, and (optionally) the installer. All build and dist folders are inside `/public`.

## Creating a Windows Installer (Inno Setup)

1. **Install Inno Setup** ([Download here](https://jrsoftware.org/isinfo.php))
2. After running `python build.py`, the installer will be created automatically if Inno Setup is installed.
3. If Inno Setup is not found, open `/public/installer.iss` in Inno Setup and click "Compile".
4. The installer (e.g., `AdvancedPDFReader-Setup-v1.0.0.exe`) will be created in `/public`.

## Usage
- Run `pdfReader.py` (or `PDFReader.exe` after build)
- Open a PDF, use the toolbar and sidebar to navigate and annotate
- **Keyboard scrolling**: Use Up/Down arrows to scroll vertically, Ctrl+Left/Right for horizontal scrolling
- **Session restore**: App automatically opens your last PDF and page when launched
- **Default PDF reader**: Set as default to open PDFs directly by double-clicking
- Use keyboard shortcuts for fast workflow (see Help > Shortcuts)

## Screenshot
![Screenshot](screenshot.png)

## License
MIT License

---

## Professional Splash and Windowing Best Practices

- Only one `Tk()` root window is ever created.
- The splash screen is a `Toplevel` child of the main (hidden) root, or the root itself if you want no flicker.
- The main window is hidden (`withdraw()`) until the splash is destroyed, then shown (`deiconify()`).
- Only one `mainloop()` is called (on the main app).
- This ensures no small/empty window flicker and a professional user experience.

---

## Sidebar Scrolling Best Practice

- Use `bindtags` to ensure mouse wheel events on thumbnails propagate to the sidebar canvas.
- Dynamically set `bindtags` for each thumbnail and the sidebar frame.

---

## /public Folder Structure

- After building, the `/public` folder contains:
  - `PDFReader-vX.Y.Z.exe` — The standalone portable executable
  - `/public/dist/` — PyInstaller distribution files
  - `/public/build/` — PyInstaller build files (can be deleted after distribution)
  - `/public/assets/` — Copied assets for the installer
  - `installer.iss` — Inno Setup script
  - `AdvancedPDFReader-Setup-vX.Y.Z.exe` — Windows installer (if Inno Setup is installed)
- Use the `/public` folder to distribute your app or upload to GitHub Releases.

---

## Setting as Default PDF Reader

### Method 1: Windows Settings
1. Right-click any PDF file
2. Select "Open with" → "Choose another app"
3. Select "Advanced PDF Reader"
4. Check "Always use this app to open .pdf files"

### Method 2: Windows Settings App
1. Open Windows Settings (`Windows + I`)
2. Go to "Apps" → "Default apps"
3. Scroll down and click "Choose default apps by file type"
4. Find ".pdf" and select "Advanced PDF Reader"

## Support
For questions or contributions, contact [needyamin@gmail.com](mailto:needyamin@gmail.com) or visit [github.com/needyamin](https://github.com/needyamin). 