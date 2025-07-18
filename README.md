# Advanced PDF Reader

A modern, professional PDF reader and annotator for Windows, built with Tkinter and PyMuPDF. Inspired by Adobe Photoshop, with a dark theme, responsive UI, annotation tools, and form fill support.

## Features
- Dark, modern UI (Photoshop-inspired)
- Sidebar with page thumbnails (active page highlighted)
- Main PDF display with zoom, fit, and rotation
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
   The standalone `.exe` and all build artifacts will be in the `/public/dist/` folder.
   The `/public` folder will contain the built `.exe`, an Inno Setup script, and (optionally) the installer. All build and dist folders are now inside `/public`.

## Creating a Windows Installer (Inno Setup)

1. **Install Inno Setup** ([Download here](https://jrsoftware.org/isinfo.php))
2. After running `python build.py`, open `/public/installer.iss` in Inno Setup and click "Compile".
3. The installer (e.g., `AdvancedPDFReader-Setup-v1.0.0.exe`) will be created in `/public`.

## Usage
- Run `pdfReader.py` (or `PDFReader.exe` after build)
- Open a PDF, use the toolbar and sidebar to navigate and annotate
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
  - `/public/dist/` — The standalone `.exe` and all distributable files
  - `/public/build/` — PyInstaller build files (can be deleted after distribution)
  - `/public/installer.iss` — Inno Setup script
  - `/public/AdvancedPDFReader-Setup-vX.Y.Z.exe` — Windows installer (if compiled)
- Use the `/public` folder to distribute your app or upload to GitHub Releases.

---

## Support
For questions or contributions, contact [needyamin@gmail.com](mailto:needyamin@gmail.com) or visit [github.com/needyamin](https://github.com/needyamin). 