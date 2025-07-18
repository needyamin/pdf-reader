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

## Screenshot
![Screenshot](screenshot.png)

## License
MIT License 

---

## **Best Practice for Reliable Sidebar Scrolling**

### 1. **Use `bindtags` to Force Event Propagation**
- Set the `bindtags` of every thumbnail widget to include the canvas, so mouse wheel events bubble up to the canvas handler.

### 2. **Bind Mouse Wheel to All Children Dynamically**
- When you create each thumbnail, set its `bindtags` so the canvas is included before `all`.

---

## **How to Fix in Your Code**

### **A. Add This Helper Function:**
```python
def _propagate_mousewheel_to_canvas(self, widget, canvas):
    # Ensure mouse wheel events on widget are handled by canvas
    tags = list(widget.bindtags())
    if canvas not in tags:
        tags.insert(1, str(canvas))
        widget.bindtags(tuple(tags))
```

### **B. When Creating Each Thumbnail:**
In your `_render_thumbnails` method, after creating each `btn` (thumbnail label), add:
```python
self._propagate_mousewheel_to_canvas(btn, self.thumbnail_canvas)
```

### **C. (Optional) Also Do This for the Sidebar Frame:**
After creating `self.thumbnail_frame`, add:
```python
self._propagate_mousewheel_to_canvas(self.thumbnail_frame, self.thumbnail_canvas)
```

---

## **Result**
- Now, when you scroll the mouse wheel over any thumbnail, the event will be handled by the sidebar canvas, and scrolling will always work as expected.

---

## **Would you like me to update your code with this fix?**  
This will make sidebar scrolling work perfectly, even when the mouse is over a thumbnail! 

---

## **How to Fix: Always Bring Main Window to Front**

You should call the following methods in your `PDFReaderApp.__init__` (after `super().__init__()` and before or after `self.title(...)`):

```python
self.lift()  # Bring window to top of stacking order
self.attributes('-topmost', True)  # Temporarily make it topmost
self.after(100, lambda: self.attributes('-topmost', False))  # Then allow other windows to go above
self.focus_force()  # Grab focus
```

This ensures:
- The main window is brought to the front.
- It gets keyboard focus.
- It does not stay always-on-top (which would be annoying).

---

## **Where to Add**

In your `PDFReaderApp.__init__`, right after `super().__init__()` and before or after `self.title('Advanced PDF Reader')`:

```python
super().__init__()
self.lift()
self.attributes('-topmost', True)
self.after(100, lambda: self.attributes('-topmost', False))
self.focus_force()
self.title('Advanced PDF Reader')
```

---

Would you like me to update your code with this fix? 

---

## Why does this happen?

- The splash screen is a separate `Tk()` window, and sometimes it appears as a blank or flickering window before the image loads.
- If the splash is not styled or sized exactly to the image, or if it briefly shows before the image is drawn, it looks bad.
- When the splash is destroyed, there can be a visible "flash" before the main window appears.

---

## How to Fix for a Professional Look

### 1. **Use `Toplevel` for Splash Instead of a Separate `Tk()`**
- Create the splash as a `Toplevel` window after the main `Tk()` is created, but keep the main window hidden (`withdraw()`) until ready.

### 2. **Hide the Main Window Until Ready**
- Use `self.withdraw()` in your main app, then `self.deiconify()` after the splash is destroyed.

### 3. **Ensure Splash is Exactly the Image Size**
- Set the splash window size to match the image, and remove all window decorations.

### 4. **No Flicker or Empty Window**
- Only show the splash after the image is loaded and geometry is set.

---

## **Example: Professional Splash Flow**

```python
<code_block_to_apply_changes_from>
```

---

## **Result**
- No small/empty window flicker.
- Splash is always the correct size and centered.
- Main window appears smoothly after splash.

---

Would you like me to update your code to use this professional splash approach? 

---

## **Why This Happens**

- In your current code, you have:
  ```python
  splash.mainloop()
  ```
  and then, after destroying the splash, you call:
  ```python
  app.mainloop()
  ```
- This is incorrect: the second `mainloop()` will block, and if you interrupt (Ctrl+C), you get a `KeyboardInterrupt`.

---

## **How to Fix**

- **Create the main app (`PDFReaderApp`) first, but keep it hidden with `withdraw()`.**
- **Create the splash as a `Toplevel` window.**
- **Call `mainloop()` only once, on the main app.**
- **When the splash is done, destroy it and call `deiconify()` on the main app.**

---

### **Correct Pattern**

```python
<code_block_to_apply_changes_from>
```

- **Only one `mainloop()`** (on the main app).
- The splash is a child window, not a separate root.
- When the splash is destroyed, the main window is shown and focused.

---

**Would you like me to update your code to use this correct, professional splash approach?** 

---

## **Why This Happens**

- When you use `tk.Tk()` for the splash, that window becomes the root window. When you destroy it and create a new `Tk()` for the main app, some systems (especially Windows) will briefly show a small empty window as the new root is initialized.
- Tkinter is not designed to have two root windows (`Tk()`), and this can cause flicker or unwanted windows.

---

## **The Only Professional Solution in Tkinter**

**You must use only one `Tk()` root window for the entire app.**  
- The splash must be a `Toplevel` child of the main (hidden) root.
- The main window must be created first, but hidden (`withdraw()`), and only shown (`deiconify()`) after the splash is destroyed.

**If you create the root after the splash, you will always get a flicker or a small window.**

---

## **How to Do It (Correct Pattern)**

1. **Create the main app (`PDFReaderApp`) first, but immediately withdraw it.**
2. **Create the splash as a `Toplevel` window.**
3. **Call `mainloop()` only once, on the main app.**
4. **When the splash is destroyed, deiconify the main app.**

**This is the only way to avoid any unwanted small window in Tkinter.**

---

### **Here is the code you should use:**

```python
<code_block_to_apply_changes_from>
```

---

**If you use this pattern, there will be NO small/empty window—only the splash, then the main GUI.**

---

Would you like me to revert your code to this correct pattern? (This is the only way to avoid the unwanted window in Tkinter.) 