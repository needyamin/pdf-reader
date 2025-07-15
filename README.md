# Advanced PDF Reader

A modern, professional PDF reader and annotator for Windows, built with Tkinter and PyMuPDF. Inspired by Adobe Photoshop, with a dark theme, responsive UI, annotation tools, and form fill support.

# Screenshots
<img width="1365" height="729" alt="Image" src="https://github.com/user-attachments/assets/92e45227-0c51-4629-ae5e-db811356574f" />

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
   The standalone `.exe` will be in the `dist/` folder.

## Usage
- Run `pdfReader.py` (or `PDFReader.exe` after build)
- Open a PDF, use the toolbar and sidebar to navigate and annotate
- Use keyboard shortcuts for fast workflow (see Help > Shortcuts)

## License
MIT License 
