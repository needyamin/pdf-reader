import tkinter as tk
from tkinter import ttk, filedialog, messagebox, simpledialog
from PIL import Image, ImageTk
import fitz  # PyMuPDF
import os
import sys
import threading
try:
    import pystray
    from PIL import Image as PILImage
except ImportError:
    pystray = None
if sys.platform == 'win32':
    try:
        from ctypes import windll
        windll.shcore.SetProcessDpiAwareness(1)
    except Exception:
        pass

# Modern dark theme colors
BG_COLOR = '#23272e'
FG_COLOR = '#f8f8f2'
ACCENT_COLOR = '#61afef'
TOOLBAR_COLOR = '#282c34'
SIDEBAR_COLOR = '#21252b'
SIDEBAR_HEADER_COLOR = '#181a1f'
STATUS_COLOR = '#181a1f'
BUTTON_ACTIVE = '#3a3f4b'
HIGHLIGHT_COLOR = '#98c379'
FONT = ('Segoe UI', 11)
FONT_BOLD = ('Segoe UI', 11, 'bold')

class Tooltip:
    def __init__(self, widget, text):
        self.widget = widget
        self.text = text
        self.tipwindow = None
        widget.bind('<Enter>', self.show)
        widget.bind('<Leave>', self.hide)
    def show(self, event=None):
        if self.tipwindow or not self.text:
            return
        x, y, _, cy = self.widget.bbox("insert") if self.widget.winfo_class() == 'Entry' else (0, 0, 0, 0)
        x = x + self.widget.winfo_rootx() + 30
        y = y + cy + self.widget.winfo_rooty() + 20
        self.tipwindow = tw = tk.Toplevel(self.widget)
        tw.wm_overrideredirect(True)
        tw.wm_geometry(f"+{x}+{y}")
        label = tk.Label(tw, text=self.text, justify='left', background='#333', foreground='white', relief='solid', borderwidth=1, font=('Segoe UI', 9))
        label.pack(ipadx=6, ipady=2)
    def hide(self, event=None):
        if self.tipwindow:
            self.tipwindow.destroy()
            self.tipwindow = None

class PDFReaderApp(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title('Advanced PDF Reader')
        self.geometry('1200x800')
        self.configure(bg=BG_COLOR)
        # Set application icon (window, taskbar, startbar)
        try:
            self.iconbitmap('icon.ico')
        except Exception:
            pass
        # Ensure app fully exits on close (no background/system tray)
        self.protocol('WM_DELETE_WINDOW', self._on_close)
        self.pdf_doc = None
        self.current_page = 0
        self.zoom = 1.0
        self.rotation = 0
        self.thumbnails = []
        self.images = []
        self._resize_after_id = None  # For debouncing
        self.fit_to_window = tk.BooleanVar(value=True)  # Default: fit to window ON
        self.sidebar_visible = True
        self._setup_style()
        self._create_widgets()
        self.after(100, self._set_default_sidebar_size)
        self.bind('<Configure>', self._on_resize)
        # Navigation
        self.bind_all('<Right>', lambda e: self.next_page())
        self.bind_all('<Left>', lambda e: self.prev_page())
        self.bind_all('<Next>', lambda e: self.next_page())  # PageDown
        self.bind_all('<Prior>', lambda e: self.prev_page())  # PageUp
        self.bind_all('<space>', lambda e: self.next_page())
        self.bind_all('<Shift-space>', lambda e: self.prev_page())
        self.bind_all('<Home>', lambda e: self.go_to_page(0))
        self.bind_all('<End>', lambda e: self.go_to_page(len(self.pdf_doc)-1 if self.pdf_doc else 0))
        # Zoom
        self.bind_all('<Control-plus>', lambda e: self.change_zoom(1.25))
        self.bind_all('<Control-equal>', lambda e: self.change_zoom(1.25))
        self.bind_all('<Control-minus>', lambda e: self.change_zoom(0.8))
        self.bind_all('<Control-underscore>', lambda e: self.change_zoom(0.8))
        self.bind_all('<Key-plus>', lambda e: self.change_zoom(1.25))
        self.bind_all('<Key-equal>', lambda e: self.change_zoom(1.25))
        self.bind_all('<Key-minus>', lambda e: self.change_zoom(0.8))
        self.bind_all('<Key-underscore>', lambda e: self.change_zoom(0.8))
        # Rotate
        self.bind_all('<r>', lambda e: self.rotate_page())
        self.bind_all('<R>', lambda e: self.rotate_page())
        # Open
        self.bind_all('<Control-o>', lambda e: self.open_pdf())
        # Fit to Window
        self.bind_all('<f>', lambda e: self.toggle_fit())
        self.bind_all('<F>', lambda e: self.toggle_fit())
        self.bind_all('<Control-f>', lambda e: self.toggle_fit())
        # Go to Page
        self.bind_all('<g>', lambda e: self.focus_page_entry())
        self.bind_all('<G>', lambda e: self.focus_page_entry())
        # Toggle Sidebar
        self.bind_all('<s>', lambda e: self.toggle_sidebar())
        self.bind_all('<S>', lambda e: self.toggle_sidebar())
        self.bind_all('<Control-b>', lambda e: self.toggle_sidebar())
        # Close
        self.bind_all('<Control-q>', lambda e: self.quit())
        self.bind_all('<Escape>', lambda e: self.quit())
        self.bind_all('<Return>', lambda e: self._goto_page_from_entry())
        self.focus_set()
        self.undo_stack = []
        self.redo_stack = []
        self.bind_all('<Control-z>', lambda e: self.undo())
        self.bind_all('<Control-y>', lambda e: self.redo())
        self.bind_all('<Delete>', lambda e: self._delete_annot_under_mouse(e))
        self.bind_all('<Control-c>', lambda e: self._copy_annot(e))
        self.bind_all('<Control-v>', lambda e: self._paste_annot(e))
        self._copied_annot_data = None

    def _setup_style(self):
        style = ttk.Style(self)
        style.theme_use('clam')
        style.configure('TFrame', background=BG_COLOR)
        style.configure('TLabel', background=BG_COLOR, foreground=FG_COLOR, font=FONT)
        style.configure('TButton', background=TOOLBAR_COLOR, foreground=FG_COLOR, borderwidth=0, focusthickness=0, focuscolor=TOOLBAR_COLOR, font=FONT)
        style.map('TButton', background=[('active', BUTTON_ACTIVE)])
        style.configure('Hover.TButton', background=BUTTON_ACTIVE, foreground=ACCENT_COLOR)

    def _create_widgets(self):
        # Menu bar
        self.menu = tk.Menu(self)
        self.config(menu=self.menu)
        # File menu
        file_menu = tk.Menu(self.menu, tearoff=0)
        file_menu.add_command(label='Open...', command=self.open_pdf, accelerator='Ctrl+O')
        file_menu.add_command(label='Save As...', command=self.save_as, accelerator='Ctrl+S')
        file_menu.add_separator()
        file_menu.add_command(label='Export as Image...', command=self.export_image)
        file_menu.add_separator()
        file_menu.add_command(label='Exit', command=self.quit, accelerator='Ctrl+Q')
        self.menu.add_cascade(label='File', menu=file_menu)
        # Startup menu
        startup_menu = tk.Menu(self.menu, tearoff=0)
        startup_menu.add_command(label='Add to Auto Startup', command=self.add_to_startup)
        self.menu.add_cascade(label='Startup', menu=startup_menu)
        # Edit menu
        edit_menu = tk.Menu(self.menu, tearoff=0)
        edit_menu.add_command(label='Undo', command=self.undo, accelerator='Ctrl+Z')
        edit_menu.add_command(label='Redo', command=self.redo, accelerator='Ctrl+Y')
        edit_menu.add_separator()
        edit_menu.add_command(label='Copy', command=self.copy, accelerator='Ctrl+C')
        edit_menu.add_command(label='Paste', command=self.paste, accelerator='Ctrl+V')
        self.menu.add_cascade(label='Edit', menu=edit_menu)
        # View menu
        view_menu = tk.Menu(self.menu, tearoff=0)
        view_menu.add_command(label='Zoom In', command=lambda: self.change_zoom(1.25), accelerator='Ctrl++')
        view_menu.add_command(label='Zoom Out', command=lambda: self.change_zoom(0.8), accelerator='Ctrl+-')
        view_menu.add_command(label='Fit to Window', command=self.toggle_fit, accelerator='F')
        view_menu.add_separator()
        view_menu.add_command(label='Toggle Sidebar', command=self.toggle_sidebar, accelerator='S')
        self.menu.add_cascade(label='View', menu=view_menu)
        # Annotate menu
        annotate_menu = tk.Menu(self.menu, tearoff=0)
        annotate_menu.add_command(label='Highlight', command=lambda: self.set_annotation_mode('highlight'))
        annotate_menu.add_command(label='Draw', command=lambda: self.set_annotation_mode('draw'))
        annotate_menu.add_command(label='Text Note', command=lambda: self.set_annotation_mode('text'))
        annotate_menu.add_command(label='Add Image', command=lambda: self.set_annotation_mode('image'))
        annotate_menu.add_command(label='Fill Form', command=lambda: self.set_annotation_mode('form'))
        annotate_menu.add_separator()
        annotate_menu.add_command(label='Eraser 🧹', command=lambda: self.set_annotation_mode('eraser'))
        self.menu.add_cascade(label='Annotate', menu=annotate_menu)
        # Help menu
        help_menu = tk.Menu(self.menu, tearoff=0)
        help_menu.add_command(label='About', command=self.show_about)
        help_menu.add_command(label='Shortcuts', command=self.show_shortcuts)
        self.menu.add_cascade(label='Help', menu=help_menu)

        # Toolbar
        self.toolbar = tk.Frame(self, bg=TOOLBAR_COLOR, height=50)
        self.toolbar.pack(side='top', fill='x')
        self._add_toolbar_buttons()

        # PanedWindow for responsive sidebar/main area
        self.paned = tk.PanedWindow(self, orient='horizontal', sashwidth=6, bg=BG_COLOR, bd=0, sashrelief='flat', showhandle=True)
        self.paned.pack(side='top', fill='both', expand=True)

        # Sidebar for thumbnails
        self.sidebar = tk.Frame(self.paned, bg=SIDEBAR_COLOR, width=140)
        self.sidebar.grid_propagate(False)
        self.sidebar_header = tk.Label(self.sidebar, text='Thumbnails', bg=SIDEBAR_HEADER_COLOR, fg=ACCENT_COLOR, font=FONT_BOLD, anchor='center')
        self.sidebar_header.pack(fill='x', pady=(0, 2))
        self.thumbnail_canvas = tk.Canvas(self.sidebar, bg=SIDEBAR_COLOR, highlightthickness=0, bd=0)
        self.thumbnail_canvas.pack(side='left', fill='both', expand=True)
        self.thumbnail_scrollbar = ttk.Scrollbar(self.sidebar, orient='vertical', command=self.thumbnail_canvas.yview)
        self.thumbnail_scrollbar.pack(side='right', fill='y')
        self.thumbnail_canvas.configure(yscrollcommand=self.thumbnail_scrollbar.set)
        self.thumbnail_frame = tk.Frame(self.thumbnail_canvas, bg=SIDEBAR_COLOR)
        self.thumbnail_canvas.create_window((0, 0), window=self.thumbnail_frame, anchor='nw')
        self.thumbnail_frame.bind('<Configure>', lambda e: self.thumbnail_canvas.configure(scrollregion=self.thumbnail_canvas.bbox('all')))
        # Sidebar scrolling and keyboard navigation
        self.thumbnail_canvas.bind_all('<MouseWheel>', self._on_sidebar_mousewheel)
        self.thumbnail_canvas.bind_all('<Button-4>', self._on_sidebar_mousewheel)  # Linux scroll up
        self.thumbnail_canvas.bind_all('<Button-5>', self._on_sidebar_mousewheel)  # Linux scroll down
        self.thumbnail_canvas.bind_all('<Up>', lambda e: self._sidebar_select_relative(-1))
        self.thumbnail_canvas.bind_all('<Down>', lambda e: self._sidebar_select_relative(1))
        self.thumbnail_canvas.bind_all('<Prior>', lambda e: self._sidebar_select_relative(-5))  # PageUp
        self.thumbnail_canvas.bind_all('<Next>', lambda e: self._sidebar_select_relative(5))   # PageDown
        self.thumbnail_canvas.bind_all('<Home>', lambda e: self.go_to_page(0))
        self.thumbnail_canvas.bind_all('<End>', lambda e: self.go_to_page(len(self.pdf_doc)-1 if self.pdf_doc else 0))
        self.thumbnail_canvas.bind_all('<Return>', lambda e: self.go_to_page(self.current_page))
        self.thumbnail_canvas.focus_set()

        # Main PDF display area with scrollbars
        self.display_outer = tk.Frame(self.paned, bg=BG_COLOR)
        self.display_outer.grid_rowconfigure(0, weight=1)
        self.display_outer.grid_columnconfigure(0, weight=1)
        self.display_frame = tk.Frame(self.display_outer, bg=BG_COLOR, highlightbackground='#444', highlightthickness=2)
        self.display_frame.grid(row=0, column=0, sticky='nsew')
        self.canvas = tk.Canvas(self.display_frame, bg=BG_COLOR, highlightthickness=0, bd=0, xscrollincrement=1, yscrollincrement=1)
        self.canvas.grid(row=0, column=0, sticky='nsew')
        self.h_scroll = ttk.Scrollbar(self.display_frame, orient='horizontal', command=self.canvas.xview)
        self.v_scroll = ttk.Scrollbar(self.display_frame, orient='vertical', command=self.canvas.yview)
        self.canvas.configure(xscrollcommand=self.h_scroll.set, yscrollcommand=self.v_scroll.set)
        self.h_scroll.grid(row=1, column=0, sticky='ew')
        self.v_scroll.grid(row=0, column=1, sticky='ns')
        self.display_frame.grid_rowconfigure(0, weight=1)
        self.display_frame.grid_columnconfigure(0, weight=1)
        # REMOVE ALL <Configure> BINDS from canvas or display_frame
        # Only bind <Configure> to the main window and debounce:
        self.bind('<Configure>', self._on_resize)

        self.paned.add(self.sidebar, minsize=80)
        self.paned.add(self.display_outer, minsize=200)
        self.paned.paneconfig(self.sidebar, stretch='never')
        self.paned.paneconfig(self.display_outer, stretch='always')

        # Status bar
        self.status = tk.Label(self, text='No document loaded', bg=STATUS_COLOR, fg=FG_COLOR, anchor='w', font=FONT)
        self.status.pack(side='bottom', fill='x')

        # Mouse zoom (Ctrl+Wheel)
        self.canvas.bind('<Control-MouseWheel>', self._on_canvas_ctrl_mousewheel)
        self.canvas.bind('<Control-Button-4>', self._on_canvas_ctrl_mousewheel)  # Linux scroll up
        self.canvas.bind('<Control-Button-5>', self._on_canvas_ctrl_mousewheel)  # Linux scroll down
        # Annotation toolbar (scaffold)
        self.annot_toolbar = tk.Frame(self.toolbar, bg=TOOLBAR_COLOR)
        self.annot_toolbar.pack(side='right', padx=8)
        self.annot_mode = tk.StringVar(value='none')
        self.annot_btns = {}
        for mode, icon, tip in [
            # ('highlight', '🖍️', 'Highlight'),
            ('draw', '✏️', 'Free Draw'),
            # ('text', '📝', 'Text Note'),
            # ('form', '🗒️', 'Fill Form'),
            ]:
            btn = ttk.Radiobutton(self.annot_toolbar, text=f'{icon} {tip if mode=="eraser" else ""}', variable=self.annot_mode, value=mode, style='TButton', command=lambda m=mode: self.set_annotation_mode(m))
            btn.pack(side='left', padx=2)
            Tooltip(btn, tip)
            self.annot_btns[mode] = btn

        # Annotation color/width pickers
        self.annot_options = tk.Frame(self.toolbar, bg=TOOLBAR_COLOR)
        self.annot_options.pack(side='right', padx=8)
        self.color_var = tk.StringVar(value='#ffff00')
        self.width_var = tk.IntVar(value=2)
        tk.Label(self.annot_options, text='Color:', bg=TOOLBAR_COLOR, fg=FG_COLOR).pack(side='left')
        self.color_btn = tk.Button(self.annot_options, bg=self.color_var.get(), width=2, command=self._pick_color)
        self.color_btn.pack(side='left', padx=2)
        tk.Label(self.annot_options, text='Width:', bg=TOOLBAR_COLOR, fg=FG_COLOR).pack(side='left')
        self.width_spin = tk.Spinbox(self.annot_options, from_=1, to=10, width=2, textvariable=self.width_var)
        self.width_spin.pack(side='left', padx=2)
        Tooltip(self.color_btn, 'Pick annotation color')
        Tooltip(self.width_spin, 'Set pen width')

    def _add_toolbar_buttons(self):
        btns = []
        def make_btn(text, cmd, icon, tooltip):
            btn = ttk.Button(self.toolbar, text=f'{icon} {text}', command=cmd, style='TButton')
            btn.pack(side='left', padx=2, pady=8)
            Tooltip(btn, tooltip)
            btn.bind('<Enter>', lambda e: btn.configure(style='Hover.TButton'))
            btn.bind('<Leave>', lambda e: btn.configure(style='TButton'))
            btns.append(btn)
            return btn
        make_btn('Open', self.open_pdf, '📂', 'Open PDF (Ctrl+O)')
        make_btn('Prev', self.prev_page, '⬅️', 'Previous Page (Left, PageUp, Shift+Space)')
        make_btn('Next', self.next_page, '➡️', 'Next Page (Right, PageDown, Space)')
        make_btn('Zoom +', lambda: self.change_zoom(1.25), '➕', 'Zoom In (Ctrl +, +, =)')
        make_btn('Zoom -', lambda:
                  self.change_zoom(0.8), '➖', 'Zoom Out (Ctrl -, -)')
        make_btn('Rotate', self.rotate_page, '🔄', 'Rotate Page (R)')
        # REMOVE the Annotate placeholder button
        # make_btn('Annotate', self.annotate_placeholder, '🖊️', 'Annotation tools (coming soon)')
        # ADD Erase button
        make_btn('Erase', lambda: self.set_annotation_mode('eraser'), '🧹', 'Eraser (Remove Annotation)')
        # Fit to Window toggle
        self.fit_btn = ttk.Checkbutton(self.toolbar, text='🖥️ Fit to Window', variable=self.fit_to_window, command=self.show_page, style='TButton')
        self.fit_btn.pack(side='left', padx=(16,2), pady=8)
        Tooltip(self.fit_btn, 'Toggle Fit to Window (F, Ctrl+F)')
        # Page number entry
        self.page_entry_var = tk.StringVar()
        self.page_entry = tk.Entry(self.toolbar, width=4, textvariable=self.page_entry_var, font=FONT, bg=TOOLBAR_COLOR, fg=FG_COLOR, insertbackground=FG_COLOR, relief='flat', justify='center')
        self.page_entry.pack(side='left', padx=(16,2), pady=8)
        Tooltip(self.page_entry, 'Go to page (G, Enter)')
        self.page_entry.bind('<FocusIn>', lambda e: self.page_entry.select_range(0, 'end'))
        self.page_entry.bind('<Return>', lambda e: self._goto_page_from_entry())
        self.page_total_label = tk.Label(self.toolbar, text='/ 0', bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT)
        self.page_total_label.pack(side='left', padx=(0,12), pady=8)
        # Form mode Done button (hidden by default)
        self.form_done_btn = ttk.Button(self.toolbar, text='✅ Done', command=self._exit_form_mode, style='TButton')
        Tooltip(self.form_done_btn, 'Exit form mode (Esc or Done)')
        self.form_done_btn.pack_forget()

    def open_pdf(self):
        file_path = filedialog.askopenfilename(filetypes=[('PDF Files', '*.pdf')])
        if not file_path:
            return
        try:
            self.pdf_doc = fitz.open(file_path)
            self.current_page = 0
            self.zoom = 1.0
            self.rotation = 0
            self.status.config(text=f'Loaded: {os.path.basename(file_path)}')
            self._render_thumbnails()
            self.fit_to_window.set(True)  # Always fit to window on open
            self.show_page()
            self.after(100, self.show_page)  # Ensure fit after canvas is ready
        except Exception as e:
            messagebox.showerror('Error', f'Failed to open PDF: {e}')

    def _render_thumbnails(self):
        for widget in self.thumbnail_frame.winfo_children():
            widget.destroy()
        self.thumbnails = []
        for i in range(len(self.pdf_doc)):
            page = self.pdf_doc.load_page(i)
            pix = page.get_pixmap(matrix=fitz.Matrix(0.18, 0.18))
            img = Image.frombytes('RGB', [pix.width, pix.height], pix.samples)
            thumb = ImageTk.PhotoImage(img)
            self.thumbnails.append(thumb)
            btn = tk.Label(self.thumbnail_frame, image=thumb, bg=SIDEBAR_COLOR, bd=2, relief='solid', highlightthickness=2)
            btn.pack(pady=4, padx=6)
            btn.bind('<Button-1>', lambda e, i=i: self.go_to_page(i))
            btn.bind('<Enter>', lambda e, b=btn: b.configure(bg=BUTTON_ACTIVE))
            btn.bind('<Leave>', lambda e, b=btn, idx=i: self._update_thumbnail_highlight(b, idx))
            # Highlight current page
            self._update_thumbnail_highlight(btn, i)

    def _update_thumbnail_highlight(self, btn, idx):
        if idx == self.current_page:
            btn.configure(bg=HIGHLIGHT_COLOR, bd=3, relief='solid', highlightbackground=ACCENT_COLOR, highlightcolor=ACCENT_COLOR)
        else:
            btn.configure(bg=SIDEBAR_COLOR, bd=2, relief='solid', highlightbackground=SIDEBAR_COLOR, highlightcolor=SIDEBAR_COLOR)

    def show_page(self):
        print(f'[DEBUG] show_page called: page={self.current_page}, zoom={self.zoom}, fit={self.fit_to_window.get()}, size=({self.canvas.winfo_width()}x{self.canvas.winfo_height()})')
        if not self.pdf_doc:
            self.page_entry_var.set('')
            self.page_total_label.config(text='/ 0')
            return
        # --- Track form widget state: (page, zoom, fit, canvas size) ---
        canvas_width = self.canvas.winfo_width()
        canvas_height = self.canvas.winfo_height()
        form_state = (self.current_page, self.zoom, self.fit_to_window.get(), canvas_width, canvas_height)
        # Remove old form entries if any, only if not in form mode or state changed ---
        if hasattr(self, '_form_entries') and (not getattr(self, 'annot_mode', None) or self.annot_mode.get() != 'form' or getattr(self, '_form_fields_active_state', None) != form_state):
            for entry in self._form_entries:
                if isinstance(entry, tuple):
                    print('[DEBUG] Destroying form widget:', type(entry[0]).__name__)
                    self.canvas.delete(entry[1])
                    entry[0].destroy()
                else:
                    print('[DEBUG] Destroying form widget:', type(entry).__name__)
                    entry.destroy()
            self._form_entries.clear()
        page = self.pdf_doc.load_page(self.current_page)
        if canvas_width < 10 or canvas_height < 10:
            return
        page_rect = page.rect
        if self.fit_to_window.get():
            zoom_x = canvas_width / page_rect.width
            zoom_y = canvas_height / page_rect.height
            zoom = min(zoom_x, zoom_y)
            mat = fitz.Matrix(zoom, zoom)
        else:
            zoom = self.zoom
            mat = fitz.Matrix(zoom, zoom)
        mat.prerotate(self.rotation)
        pix = page.get_pixmap(matrix=mat)
        img = Image.frombytes('RGB', [pix.width, pix.height], pix.samples)
        if self.fit_to_window.get():
            img_ratio = img.width / img.height
            canvas_ratio = canvas_width / canvas_height
            if img_ratio > canvas_ratio:
                new_width = canvas_width
                new_height = int(canvas_width / img_ratio)
            else:
                new_height = canvas_height
                new_width = int(canvas_height * img_ratio)
            img = img.resize((new_width, new_height), Image.LANCZOS)
            self._pdf_scale = new_width / page_rect.width
            self._pdf_offset_x = (canvas_width - new_width) // 2
            self._pdf_offset_y = (canvas_height - new_height) // 2
            self._pdf_render_width = new_width
            self._pdf_render_height = new_height
            self.images = [ImageTk.PhotoImage(img)]
            self.canvas.config(scrollregion=(0,0,new_width,new_height))
            self.canvas.delete('all')
            self.canvas.create_image(canvas_width//2, canvas_height//2, image=self.images[0], anchor='center')
            self.canvas.config(xscrollcommand=self.h_scroll.set, yscrollcommand=self.v_scroll.set)
            self.h_scroll.grid_remove()
            self.v_scroll.grid_remove()
        else:
            self._pdf_scale = zoom
            self._pdf_offset_x = 0
            self._pdf_offset_y = 0
            self._pdf_render_width = img.width
            self._pdf_render_height = img.height
            self.images = [ImageTk.PhotoImage(img)]
            self.canvas.config(scrollregion=(0,0,img.width,img.height))
            self.canvas.delete('all')
            self.canvas.create_image(0, 0, image=self.images[0], anchor='nw')
            self.h_scroll.grid()
            self.v_scroll.grid()
        self.page_entry_var.set(str(self.current_page+1))
        self.page_total_label.config(text=f'/ {len(self.pdf_doc)}')
        fname = self.pdf_doc.name if hasattr(self.pdf_doc, 'name') else ''
        self.status.config(text=f'{os.path.basename(fname)} | Page {self.current_page+1}/{len(self.pdf_doc)} | Zoom: {int(self.zoom*100)}% | Rotation: {self.rotation}° | Fit: {"ON" if self.fit_to_window.get() else "OFF"}')
        for idx, widget in enumerate(self.thumbnail_frame.winfo_children()):
            self._update_thumbnail_highlight(widget, idx)
        # --- Only enable form fields if in form mode and not already active for this state ---
        if getattr(self, 'annot_mode', None) and self.annot_mode.get() == 'form':
            if getattr(self, '_form_fields_active_state', None) != form_state:
                self._enable_form_fields()
                self._form_fields_active_state = form_state

    def prev_page(self):
        if self.pdf_doc and self.current_page > 0:
            self.current_page -= 1
            self.show_page()

    def next_page(self):
        if self.pdf_doc and self.current_page < len(self.pdf_doc) - 1:
            self.current_page += 1
            self.show_page()

    def go_to_page(self, page_num):
        if self.pdf_doc and 0 <= page_num < len(self.pdf_doc):
            self.current_page = page_num
            self.show_page()

    def _goto_page_from_entry(self):
        if not self.pdf_doc:
            return
        try:
            page = int(self.page_entry_var.get()) - 1
            if 0 <= page < len(self.pdf_doc):
                self.current_page = page
                self.show_page()
        except Exception:
            pass

    def change_zoom(self, factor):
        if self.pdf_doc:
            if self.fit_to_window.get():
                self.fit_to_window.set(False)
            self.zoom *= factor
            self.zoom = max(0.1, min(self.zoom, 10.0))
            self.show_page()  # Always update page and mapping after zoom

    def rotate_page(self):
        if self.pdf_doc:
            self.rotation = (self.rotation + 90) % 360
            self.show_page()

    def annotate_placeholder(self):
        messagebox.showinfo('Annotation', 'Annotation tools coming soon!')

    def _on_resize(self, event):
        # Debounce: only redraw after resizing stops for 100ms
        if self._resize_after_id:
            self.after_cancel(self._resize_after_id)
        self._resize_after_id = self.after(100, self._debounced_resize)

    def _debounced_resize(self):
        self._resize_after_id = None
        self.show_page()
        self.thumbnail_canvas.configure(scrollregion=self.thumbnail_canvas.bbox('all'))

    def _set_default_sidebar_size(self):
        # Set sidebar to a reasonable width (e.g., 180px) on startup
        try:
            self.paned.sash_place(0, 180, 0)
        except Exception:
            pass

    def toggle_fit(self):
        self.fit_to_window.set(not self.fit_to_window.get())
        self.show_page()  # Always update page and mapping after fit toggle

    def focus_page_entry(self):
        self.page_entry.focus_set()
        self.page_entry.select_range(0, 'end')

    def toggle_sidebar(self):
        if self.sidebar_visible:
            self.paned.forget(self.sidebar)
            self.sidebar_visible = False
        else:
            self.paned.insert(0, self.sidebar)
            self.sidebar_visible = True
        self.show_page()

    def _on_sidebar_mousewheel(self, event):
        # Windows/Mac: event.delta, Linux: event.num
        if event.num == 4 or event.delta > 0:
            self.thumbnail_canvas.yview_scroll(-3, 'units')
        elif event.num == 5 or event.delta < 0:
            self.thumbnail_canvas.yview_scroll(3, 'units')

    def _sidebar_select_relative(self, offset):
        if not self.pdf_doc:
            return
        new_page = min(max(self.current_page + offset, 0), len(self.pdf_doc)-1)
        if new_page != self.current_page:
            self.go_to_page(new_page)
            # Scroll sidebar to keep selected thumbnail visible
            widget = self.thumbnail_frame.winfo_children()[new_page]
            self.thumbnail_canvas.yview_moveto(widget.winfo_y() / max(1, self.thumbnail_frame.winfo_height()))

    def _on_canvas_ctrl_mousewheel(self, event):
        if event.num == 4 or event.delta > 0:
            self.change_zoom(1.1)
        elif event.num == 5 or event.delta < 0:
            self.change_zoom(0.9)

    def save_as(self):
        if not self.pdf_doc:
            messagebox.showwarning('No PDF', 'No PDF loaded.')
            return
        file_path = filedialog.asksaveasfilename(defaultextension='.pdf', filetypes=[('PDF Files', '*.pdf')])
        if file_path:
            self.pdf_doc.save(file_path)
            messagebox.showinfo('Saved', f'Saved as {file_path}')

    def export_image(self):
        if not self.pdf_doc:
            messagebox.showwarning('No PDF', 'No PDF loaded.')
            return
        file_path = filedialog.asksaveasfilename(defaultextension='.png', filetypes=[('PNG Image', '*.png')])
        if file_path:
            page = self.pdf_doc.load_page(self.current_page)
            pix = page.get_pixmap()
            pix.save(file_path)
            messagebox.showinfo('Exported', f'Page exported as {file_path}')

    def _push_undo(self, action, data=None):
        self.undo_stack.append((action, data))
        self.redo_stack.clear()
        self.status.config(text='Action added to undo stack.')

    def _push_redo(self, action, data=None):
        self.redo_stack.append((action, data))

    def undo(self):
        if not self.undo_stack:
            self.status.config(text='Nothing to undo.')
            return
        action, data = self.undo_stack.pop()
        if action == 'add_annot':
            page = self.pdf_doc.load_page(data['page'])
            for xref in data['annots']:
                try:
                    page.delete_annot(page.load_annot(xref))
                except Exception:
                    pass
            self._push_redo('remove_annot', data)
            self.pdf_doc.saveIncr()
            self.show_page()
            self.status.config(text='Undo: Annotation removed.')
        elif action == 'remove_annot':
            # Not implemented: would require full annotation serialization
            self.status.config(text='Undo: Cannot re-add annotation (not implemented).')
        elif action == 'form_fill':
            page = self.pdf_doc.load_page(data['page'])
            widget = data['widget']
            widget.field_value = data['old_value']
            widget.update()
            self._push_redo('form_fill', {'page': data['page'], 'widget': widget, 'old_value': data['new_value'], 'new_value': data['old_value']})
            self.pdf_doc.saveIncr()
            self.show_page()
            self.status.config(text='Undo: Form fill reverted.')
        else:
            self.status.config(text='Undo: Not implemented for this action.')

    def redo(self):
        if not self.redo_stack:
            self.status.config(text='Nothing to redo.')
            return
        action, data = self.redo_stack.pop()
        if action == 'remove_annot':
            page = self.pdf_doc.load_page(data['page'])
            for xref in data['annots']:
                try:
                    page.delete_annot(page.load_annot(xref))
                except Exception:
                    pass
            self._push_undo('add_annot', data)
            self.pdf_doc.saveIncr()
            self.show_page()
            self.status.config(text='Redo: Annotation removed again.')
        elif action == 'form_fill':
            page = self.pdf_doc.load_page(data['page'])
            widget = data['widget']
            widget.field_value = data['new_value']
            widget.update()
            self._push_undo('form_fill', {'page': data['page'], 'widget': widget, 'old_value': data['old_value'], 'new_value': data['new_value']})
            self.pdf_doc.saveIncr()
            self.show_page()
            self.status.config(text='Redo: Form fill applied again.')
        else:
            self.status.config(text='Redo: Not implemented for this action.')

    def copy(self):
        messagebox.showinfo('Copy', 'Copy not implemented yet.')
    def paste(self):
        messagebox.showinfo('Paste', 'Paste not implemented yet.')

    def _pick_color(self):
        import tkinter.colorchooser as cc
        color = cc.askcolor(color=self.color_var.get())[1]
        if color:
            self.color_var.set(color)
            self.color_btn.config(bg=color)

    def set_annotation_mode(self, mode):
        prev_mode = self.annot_mode.get() if hasattr(self, 'annot_mode') else None
        self.annot_mode.set(mode)
        for m, btn in self.annot_btns.items():
            if m == mode:
                btn.configure(style='Hover.TButton')
            else:
                btn.configure(style='TButton')
        if mode == 'eraser':
            self.status.config(text='Eraser: Click on an annotation to remove it.')
        elif mode == 'form':
            self.status.config(text='Form fields enabled. Fill and click Done or press Esc to finish.')
            self.form_done_btn.pack(side='right', padx=8)
            self.bind_all('<Escape>', lambda e: self._exit_form_mode())
        else:
            self.status.config(text=f'Annotation mode: {mode.capitalize()}')
            self.form_done_btn.pack_forget()
            self.unbind_all('<Escape>')
        if mode in ('highlight', 'draw'):
            self.annot_options.pack(side='right', padx=8)
        else:
            self.annot_options.pack_forget()
        self.canvas.unbind('<Button-1>')
        self.canvas.unbind('<B1-Motion>')
        self.canvas.unbind('<ButtonRelease-1>')
        self.canvas.config(cursor='arrow')
        if mode == 'highlight':
            self.canvas.bind('<Button-1>', self._start_text_highlight)
            self.canvas.config(cursor='xterm')
        elif mode == 'draw':
            self.canvas.bind('<Button-1>', self._start_draw)
            self.canvas.bind('<B1-Motion>', self._draw_draw)
            self.canvas.bind('<ButtonRelease-1>', self._end_draw)
            self.canvas.config(cursor='pencil')
        elif mode == 'text':
            self.canvas.bind('<Button-1>', self._add_text_note)
            self.canvas.config(cursor='plus')
        elif mode == 'image':
            self.canvas.bind('<Button-1>', self._start_image_annot)
            self.canvas.bind('<B1-Motion>', self._resize_image_annot)
            self.canvas.bind('<ButtonRelease-1>', self._end_image_annot)
            self.canvas.config(cursor='plus')
        elif mode == 'form':
            # Only enable form fields if not already active for this state
            canvas_width = self.canvas.winfo_width()
            canvas_height = self.canvas.winfo_height()
            form_state = (self.current_page, self.zoom, self.fit_to_window.get(), canvas_width, canvas_height)
            if getattr(self, '_form_fields_active_state', None) != form_state:
                self._enable_form_fields()
                self._form_fields_active_state = form_state
        elif mode == 'eraser':
            self.canvas.bind('<Button-1>', self._erase_annot)
            self.canvas.bind('<Motion>', self._highlight_eraser_hover)
            self.canvas.config(cursor='dotbox')
        else:
            self._disable_form_fields()
            self.canvas.config(cursor='arrow')
        # Remove old form entries if not in form mode
        if mode != 'form' and hasattr(self, '_form_entries'):
            for entry in self._form_entries:
                if isinstance(entry, tuple):
                    self.canvas.delete(entry[1])
                    entry[0].destroy()
                else:
                    entry.destroy()
            self._form_entries.clear()
            self._form_fields_active_state = None

    # --- Coordinate conversion helpers ---
    def canvas_to_pdf(self, x, y):
        """Convert canvas coordinates to PDF page coordinates."""
        px = (x - self._pdf_offset_x) / self._pdf_scale
        py = (y - self._pdf_offset_y) / self._pdf_scale
        return px, py
    def pdf_to_canvas(self, x, y):
        """Convert PDF page coordinates to canvas coordinates."""
        cx = x * self._pdf_scale + self._pdf_offset_x
        cy = y * self._pdf_scale + self._pdf_offset_y
        return cx, cy

    # --- Update annotation tools to use coordinate conversion ---
    # --- Professional Highlight Tool (text selection, click-drag) ---
    def _start_text_highlight(self, event):
        cx, cy = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        print(f'[DEBUG] Highlight start canvas: ({cx}, {cy}), scale: {getattr(self, "_pdf_scale", None)}, offset: ({getattr(self, "_pdf_offset_x", None)}, {getattr(self, "_pdf_offset_y", None)})')
        self._highlight_start = (cx, cy)
        self.canvas.bind('<B1-Motion>', self._drag_text_highlight)
        self.canvas.bind('<ButtonRelease-1>', self._end_text_highlight)
        self._highlight_rect = self.canvas.create_rectangle(self._highlight_start[0], self._highlight_start[1], self._highlight_start[0], self._highlight_start[1], outline=HIGHLIGHT_COLOR, width=2, dash=(2,2))
    def _drag_text_highlight(self, event):
        x0, y0 = self._highlight_start
        x1, y1 = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        self.canvas.coords(self._highlight_rect, x0, y0, x1, y1)
    def _end_text_highlight(self, event):
        x0, y0 = self._highlight_start
        x1, y1 = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        self.canvas.delete(self._highlight_rect)
        self.canvas.unbind('<B1-Motion>')
        self.canvas.unbind('<ButtonRelease-1>')
        # Convert to PDF coordinates
        px0, py0 = self.canvas_to_pdf(x0, y0)
        px1, py1 = self.canvas_to_pdf(x1, y1)
        rect = fitz.Rect(min(px0, px1), min(py0, py1), max(px0, px1), max(py0, py1))
        page = self.pdf_doc.load_page(self.current_page)
        # Find all words in the rectangle and highlight them
        words = page.get_text('words')
        added = False
        annots_added = []
        for w in words:
            wx0, wy0, wx1, wy1, word, *_ = w
            word_rect = fitz.Rect(wx0, wy0, wx1, wy1)
            if rect.intersects(word_rect):
                annot = page.add_highlight_annot(word_rect)
                annot.set_colors(stroke=self.color_var.get(), fill=self.color_var.get())
                annot.update()
                annots_added.append(annot.xref)
                added = True
        if added:
            self._push_undo('add_annot', {'page': self.current_page, 'annots': annots_added})
            self.pdf_doc.saveIncr()
            self.show_page()
        self._highlight_start = None
        self._highlight_rect = None

    def _start_draw(self, event):
        cx, cy = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        print(f'[DEBUG] Draw start canvas: ({cx}, {cy}), scale: {getattr(self, "_pdf_scale", None)}, offset: ({getattr(self, "_pdf_offset_x", None)}, {getattr(self, "_pdf_offset_y", None)})')
        self._draw_points = [(cx, cy)]
        self._draw_line = self.canvas.create_line(cx, cy, cx, cy, fill=self.color_var.get(), width=self.width_var.get())
        self._draw_history = []
    def _draw_draw(self, event):
        if hasattr(self, '_draw_line'):
            cx, cy = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
            self._draw_points.append((cx, cy))
            self.canvas.coords(self._draw_line, *sum(self._draw_points, ()))
    def _end_draw(self, event):
        if hasattr(self, '_draw_line'):
            page = self.pdf_doc.load_page(self.current_page)
            # Convert all points to PDF coordinates
            pdf_points = [self.canvas_to_pdf(x, y) for x, y in self._draw_points]
            annot = page.add_ink_annot([pdf_points])
            annot.set_colors(stroke=self.color_var.get())
            annot.set_border(width=self.width_var.get())
            annot.update()
            self._push_undo('add_annot', {'page': self.current_page, 'annots': [annot.xref]})
            self.pdf_doc.saveIncr()
            self.show_page()
            self._draw_history.append(self._draw_line)
            del self._draw_line
            del self._draw_points

    # --- Professional Form Fill ---
    def _enable_form_fields(self):
        print(f'[DEBUG] _enable_form_fields called for page {self.current_page}')
        page = self.pdf_doc.load_page(self.current_page)
        widgets = list(page.widgets())
        if not hasattr(self, '_form_entries'):
            self._form_entries = []
        else:
            for entry in self._form_entries:
                if isinstance(entry, tuple):
                    print('[DEBUG] Destroying form widget:', type(entry[0]).__name__)
                    self.canvas.delete(entry[1])
                    entry[0].destroy()
                else:
                    print('[DEBUG] Destroying form widget:', type(entry).__name__)
                    entry.destroy()
            self._form_entries.clear()
        if not widgets:
            print('[DEBUG] No form widgets detected on this page.')
            return
        for widget in widgets:
            rect = widget.rect
            x0, y0 = self.pdf_to_canvas(rect.x0, rect.y0)
            x1, y1 = self.pdf_to_canvas(rect.x1, rect.y1)
            w, h = x1 - x0, y1 - y0
            if widget.field_type == getattr(fitz, 'PDF_WIDGET_TYPE_TEXT', 4096):
                print(f'[DEBUG] Creating Entry at ({x0},{y0}) size ({w}x{h})')
                entry = tk.Entry(self.canvas, width=20, relief='solid', bd=2, highlightthickness=2, highlightbackground=ACCENT_COLOR, highlightcolor=ACCENT_COLOR)
                entry.insert(0, widget.field_value or '')
                win = self.canvas.create_window(x0, y0, anchor='nw', window=entry, width=w, height=h)
                entry.lift()
                entry.bind('<FocusIn>', lambda e, ent=entry: ent.config(highlightbackground=HIGHLIGHT_COLOR))
                entry.bind('<FocusOut>', lambda e, w=widget, ent=entry: self._save_form_field(w, ent, refresh=False))
                entry.bind('<Return>', lambda e, w=widget, ent=entry: self._save_form_field(w, ent, refresh=False))
                self._form_entries.append((entry, win))
            elif widget.field_type == getattr(fitz, 'PDF_WIDGET_TYPE_CHECKBOX', 32768):
                print(f'[DEBUG] Creating Checkbutton at ({x0},{y0}) size ({w}x{h})')
                export_value = getattr(widget, 'export_value', None) or 'Yes'
                is_checked = (widget.field_value == export_value or widget.field_value == 'On' or widget.field_value is True)
                var = tk.BooleanVar(value=is_checked)
                cb = tk.Checkbutton(self.canvas, variable=var, bg=BG_COLOR, activebackground=BG_COLOR, selectcolor=ACCENT_COLOR, relief='solid', bd=2, highlightthickness=2)
                win = self.canvas.create_window(x0, y0, anchor='nw', window=cb, width=w, height=h)
                cb.lift()
                def on_cb_toggle(v=var, w=widget, export_value=export_value, cb=cb):
                    try:
                        if hasattr(w, 'set_checked'):
                            w.set_checked(v.get())
                        else:
                            if v.get():
                                w.field_value = export_value
                            else:
                                w.field_value = None
                        w.update()
                        self.pdf_doc.saveIncr()
                        checked = (w.field_value == export_value or w.field_value == 'On' or w.field_value is True)
                        v.set(checked)
                        print(f'[DEBUG] Checkbox toggled: {checked}')
                    except Exception as ex:
                        print('[DEBUG] Checkbox toggle error:', ex)
                var.trace_add('write', lambda *a, v=var, w=widget, export_value=export_value, cb=cb: on_cb_toggle(v, w, export_value, cb))
                self._form_entries.append((cb, win))
            elif widget.field_type == getattr(fitz, 'PDF_WIDGET_TYPE_RADIO', 65536):
                print(f'[DEBUG] Creating Radiobutton at ({x0},{y0}) size ({w}x{h})')
                if not hasattr(self, '_radio_vars'):
                    self._radio_vars = {}
                if widget.field_name not in self._radio_vars:
                    self._radio_vars[widget.field_name] = tk.StringVar(value=widget.field_value or '')
                var = self._radio_vars[widget.field_name]
                rb = tk.Radiobutton(self.canvas, variable=var, value=widget.field_value or widget.field_label or 'On', bg=BG_COLOR, activebackground=BG_COLOR, selectcolor=ACCENT_COLOR, relief='solid', bd=2, highlightthickness=2)
                win = self.canvas.create_window(x0, y0, anchor='nw', window=rb, width=w, height=h)
                rb.lift()
                def on_rb_toggle(v=var, w=widget):
                    w.field_value = v.get()
                    w.update()
                    self.pdf_doc.saveIncr()
                    print(f'[DEBUG] Radiobutton toggled: {v.get()}')
                var.trace_add('write', lambda *a, v=var, w=widget: on_rb_toggle(v, w))
                self._form_entries.append((rb, win))
            elif widget.field_type == getattr(fitz, 'PDF_WIDGET_TYPE_COMBO', 131072):
                print(f'[DEBUG] Creating Combobox at ({x0},{y0}) size ({w}x{h})')
                values = widget.field_options or []
                var = tk.StringVar(value=widget.field_value or (values[0] if values else ''))
                combo = ttk.Combobox(self.canvas, textvariable=var, values=values, font=FONT)
                win = self.canvas.create_window(x0, y0, anchor='nw', window=combo, width=w, height=h)
                combo.lift()
                def on_combo_select(event, v=var, w=widget):
                    w.field_value = v.get()
                    w.update()
                    self.pdf_doc.saveIncr()
                    print(f'[DEBUG] Combobox selected: {v.get()}')
                combo.bind('<<ComboboxSelected>>', on_combo_select)
                self._form_entries.append((combo, win))
            elif widget.field_type == getattr(fitz, 'PDF_WIDGET_TYPE_LIST', 262144):
                print(f'[DEBUG] Creating Listbox at ({x0},{y0}) size ({w}x{h})')
                values = widget.field_options or []
                var = tk.StringVar(value=widget.field_value or (values[0] if values else ''))
                lb = tk.Listbox(self.canvas, listvariable=tk.StringVar(value=values), selectmode='single', font=FONT)
                win = self.canvas.create_window(x0, y0, anchor='nw', window=lb, width=w, height=h)
                lb.lift()
                def on_lb_select(event, v=var, w=widget, lb=lb):
                    sel = lb.curselection()
                    if sel:
                        w.field_value = lb.get(sel[0])
                        w.update()
                        self.pdf_doc.saveIncr()
                        print(f'[DEBUG] Listbox selected: {lb.get(sel[0])}')
                lb.bind('<<ListboxSelect>>', on_lb_select)
                self._form_entries.append((lb, win))

    def _save_form_field(self, widget, entry, refresh=False):
        widget.field_value = entry.get()
        widget.update()
        self.pdf_doc.saveIncr()
        # Do NOT call show_page() here, even if refresh=True
        # Only destroy/recreate widgets on navigation, zoom, fit, or explicit refresh

    def _highlight_eraser_hover(self, event):
        # Visual feedback: highlight annotation under cursor
        page = self.pdf_doc.load_page(self.current_page)
        x, y = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        found = None
        for annot in page.annots():
            rect = annot.rect
            if rect.contains(fitz.Point(x, y)):
                found = rect
                break
        # Remove previous highlight
        if hasattr(self, '_eraser_hover_rect'):
            self.canvas.delete(self._eraser_hover_rect)
            del self._eraser_hover_rect
        if found:
            # Draw highlight rectangle
            self._eraser_hover_rect = self.canvas.create_rectangle(
                found.x0, found.y0, found.x1, found.y1,
                outline='red', width=2, dash=(3,2))

    def _erase_annot(self, event):
        # Find and delete annotation under cursor
        page = self.pdf_doc.load_page(self.current_page)
        x = self.canvas.canvasx(self.winfo_pointerx() - self.canvas.winfo_rootx())
        y = self.canvas.canvasy(self.winfo_pointery() - self.canvas.winfo_rooty())
        found = False
        for annot in page.annots():
            rect = annot.rect
            if rect.contains(fitz.Point(*self.canvas_to_pdf(x, y))):
                self._push_undo('remove_annot', {'page': self.current_page, 'annots': [annot.xref]})
                page.delete_annot(annot)
                found = True
                break
        if hasattr(self, '_eraser_hover_rect'):
            self.canvas.delete(self._eraser_hover_rect)
            del self._eraser_hover_rect
        if found:
            self.pdf_doc.saveIncr()
            self.show_page()
        else:
            messagebox.showinfo('Eraser', 'No annotation found at this location.')

    # --- Professional Image Tool: Preview, Resize, Move Before Finalize ---
    def _start_image_annot(self, event):
        cx, cy = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        print(f'[DEBUG] Image annot start canvas: ({cx}, {cy}), scale: {getattr(self, "_pdf_scale", None)}, offset: ({getattr(self, "_pdf_offset_x", None)}, {getattr(self, "_pdf_offset_y", None)})')
        self._img_start = (cx, cy)
        self._img_rect = self.canvas.create_rectangle(cx, cy, cx, cy, outline=ACCENT_COLOR, width=2, dash=(2,2))
        self._img_preview = None
        self._img_preview_path = None
        self.canvas.bind('<Double-Button-1>', self._finalize_image_annot)
        self.canvas.bind('<Return>', self._finalize_image_annot)
    def _resize_image_annot(self, event):
        if hasattr(self, '_img_rect'):
            cx, cy = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
            self.canvas.coords(self._img_rect, self._img_start[0], self._img_start[1], cx, cy)
    def _end_image_annot(self, event):
        if hasattr(self, '_img_rect'):
            file_path = filedialog.askopenfilename(filetypes=[('Image Files', '*.png;*.jpg;*.jpeg;*.bmp')])
            if not file_path:
                self.canvas.delete(self._img_rect)
                del self._img_rect
                del self._img_start
                return
            self._img_preview_path = file_path
            x0, y0 = self._img_start
            x1, y1 = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
            self._img_box = (min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1))
            # Show preview image on canvas
            from PIL import Image as PILImage
            img = PILImage.open(file_path)
            w = max(1, int(self._img_box[2] - self._img_box[0]))
            h = max(1, int(self._img_box[3] - self._img_box[1]))
            img = img.resize((w, h), PILImage.LANCZOS)
            self._img_preview_img = ImageTk.PhotoImage(img)
            if self._img_preview:
                self.canvas.delete(self._img_preview)
            self._img_preview = self.canvas.create_image(self._img_box[0], self._img_box[1], image=self._img_preview_img, anchor='nw')
            # Allow drag/resize: bind mouse events
            self.canvas.bind('<B1-Motion>', self._drag_image_annot)
            self.canvas.bind('<Button-1>', self._select_image_annot)
    def _select_image_annot(self, event):
        # Start drag
        self._drag_offset = (self.canvas.canvasx(event.x) - self._img_box[0], self.canvas.canvasy(event.y) - self._img_box[1])
    def _drag_image_annot(self, event):
        # Move preview image
        if hasattr(self, '_img_preview'):
            x = self.canvas.canvasx(event.x) - self._drag_offset[0]
            y = self.canvas.canvasy(event.y) - self._drag_offset[1]
            w = self._img_box[2] - self._img_box[0]
            h = self._img_box[3] - self._img_box[1]
            self._img_box = (x, y, x + w, y + h)
            self.canvas.coords(self._img_preview, x, y)
            self.canvas.coords(self._img_rect, x, y, x + w, y + h)
    def _finalize_image_annot(self, event=None):
        if hasattr(self, '_img_preview') and self._img_preview_path:
            px0, py0 = self.canvas_to_pdf(self._img_box[0], self._img_box[1])
            px1, py1 = self.canvas_to_pdf(self._img_box[2], self._img_box[3])
            rect = fitz.Rect(min(px0,px1), min(py0,py1), max(px0,px1), max(py0,py1))
            page = self.pdf_doc.load_page(self.current_page)
            pix = fitz.Pixmap(self._img_preview_path)
            annot = page.insert_image(rect, pixmap=pix, keep_proportion=False, overlay=True, xref=None, rotate=0, mask=None, alpha=False, annots=True)
            # PyMuPDF's insert_image does not return an annotation, so we need to find the last image annotation
            last_annot = None
            for a in page.annots():
                last_annot = a
            if last_annot:
                self._push_undo('add_annot', {'page': self.current_page, 'annots': [last_annot.xref]})
            self.pdf_doc.saveIncr()
            self.show_page()
            self.canvas.delete(self._img_rect)
            self.canvas.delete(self._img_preview)
            del self._img_rect
            del self._img_preview
            del self._img_box
            del self._img_preview_path
            del self._img_preview_img
            self.canvas.unbind('<B1-Motion>')
            self.canvas.unbind('<Button-1>')
            self.canvas.unbind('<Double-Button-1>')
            self.canvas.unbind('<Return>')

    # --- Professional Text Note Tool: Resizable, Movable, Editable Before Finalize ---
    def _add_text_note(self, event):
        cx, cy = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
        print(f'[DEBUG] Text note start canvas: ({cx}, {cy}), scale: {getattr(self, "_pdf_scale", None)}, offset: ({getattr(self, "_pdf_offset_x", None)}, {getattr(self, "_pdf_offset_y", None)})')
        self._text_box = [cx, cy, cx+120, cy+40]  # Default size
        self._text_rect = self.canvas.create_rectangle(*self._text_box, outline=ACCENT_COLOR, width=2, dash=(2,2))
        self._text_entry = tk.Text(self.canvas, width=20, height=2, font=FONT, bg=BG_COLOR, fg=FG_COLOR, insertbackground=FG_COLOR, relief='solid', bd=2, highlightthickness=2, highlightbackground=ACCENT_COLOR, highlightcolor=ACCENT_COLOR)
        self._text_entry.place(x=cx, y=cy, width=120, height=40)
        self._text_entry.focus_set()
        self._text_entry.bind('<B1-Motion>', self._resize_text_note)
        self._text_entry.bind('<Double-Button-1>', self._finalize_text_note)
        self._text_entry.bind('<Return>', self._finalize_text_note)
        self._text_entry.bind('<Escape>', self._cancel_text_note)
        self._text_entry.bind('<FocusIn>', lambda e: self._text_entry.config(highlightbackground=HIGHLIGHT_COLOR))
        self._text_entry.bind('<FocusOut>', lambda e: self._text_entry.config(highlightbackground=ACCENT_COLOR))
        self._text_drag_offset = None
        self._text_entry.bind('<Button-1>', self._start_drag_text_note)
    def _start_drag_text_note(self, event):
        self._text_drag_offset = (event.x, event.y)
        self._text_entry.bind('<B1-Motion>', self._drag_text_note)
    def _drag_text_note(self, event):
        dx = event.x - self._text_drag_offset[0]
        dy = event.y - self._text_drag_offset[1]
        x, y, w, h = self._text_entry.winfo_x(), self._text_entry.winfo_y(), self._text_entry.winfo_width(), self._text_entry.winfo_height()
        new_x = x + dx
        new_y = y + dy
        self._text_entry.place(x=new_x, y=new_y)
        self.canvas.coords(self._text_rect, new_x, new_y, new_x + w, new_y + h)
    def _resize_text_note(self, event):
        # Resize box by dragging lower right corner
        x, y = self._text_entry.winfo_x(), self._text_entry.winfo_y()
        w, h = max(40, event.x), max(20, event.y)
        self._text_entry.place(x=x, y=y, width=w, height=h)
        self.canvas.coords(self._text_rect, x, y, x + w, y + h)
    def _finalize_text_note(self, event=None):
        text = self._text_entry.get('1.0', 'end').strip()
        if text:
            x, y = self._text_entry.winfo_x(), self._text_entry.winfo_y()
            w, h = self._text_entry.winfo_width(), self._text_entry.winfo_height()
            px, py = self.canvas_to_pdf(x, y)
            page = self.pdf_doc.load_page(self.current_page)
            annot = page.add_freetext_annot(fitz.Rect(px, py, px + w/self._pdf_scale, py + h/self._pdf_scale), text, fontsize=12, fontname="helv", align=0)
            annot.set_colors(stroke=self.color_var.get(), fill=None)
            annot.update()
            self._push_undo('add_annot', {'page': self.current_page, 'annots': [annot.xref]})
            self.pdf_doc.saveIncr()
            self.show_page()
        self.canvas.delete(self._text_rect)
        self._text_entry.destroy()
        del self._text_rect
        del self._text_entry
        if hasattr(self, '_text_drag_offset'):
            del self._text_drag_offset
    def _cancel_text_note(self, event=None):
        self.canvas.delete(self._text_rect)
        self._text_entry.destroy()
        del self._text_rect
        del self._text_entry
        if hasattr(self, '_text_drag_offset'):
            del self._text_drag_offset

    def show_about(self):
        messagebox.showinfo('About', 'Advanced PDF Reader\nPowered by Tkinter, PyMuPDF, Pillow')
    def show_shortcuts(self):
        shortcuts = [
            ('Open PDF', 'Ctrl+O'),
            ('Save As', 'Ctrl+S'),
            ('Export as Image', ''),
            ('Exit', 'Ctrl+Q, Esc'),
            ('Undo', 'Ctrl+Z'),
            ('Redo', 'Ctrl+Y'),
            ('Copy', 'Ctrl+C'),
            ('Paste', 'Ctrl+V'),
            ('Next Page', 'Right, PageDown, Space'),
            ('Previous Page', 'Left, PageUp, Shift+Space'),
            ('First Page', 'Home'),
            ('Last Page', 'End'),
            ('Zoom In', 'Ctrl++, +, ='),
            ('Zoom Out', 'Ctrl+-, -'),
            ('Rotate', 'R'),
            ('Fit to Window', 'F, Ctrl+F'),
            ('Go to Page', 'G, Enter'),
            ('Toggle Sidebar', 'S, Ctrl+B'),
            # ('Highlight', 'Annotate > Highlight'),
            ('Draw', 'Annotate > Draw'),
            # ('Text Note', 'Annotate > Text Note'),
            # ('Add Image', 'Annotate > Add Image'),
            # ('Fill Form', 'Annotate > Fill Form'),
            ('Eraser', 'Annotate > Eraser'),
        ]
        top = tk.Toplevel(self)
        top.title('Keyboard Shortcuts')
        top.configure(bg=BG_COLOR)
        top.geometry('480x520')
        frame = tk.Frame(top, bg=BG_COLOR)
        frame.pack(fill='both', expand=True, padx=16, pady=16)
        canvas = tk.Canvas(frame, bg=BG_COLOR, highlightthickness=0)
        scrollbar = ttk.Scrollbar(frame, orient='vertical', command=canvas.yview)
        scroll_frame = tk.Frame(canvas, bg=BG_COLOR)
        scroll_frame.bind('<Configure>', lambda e: canvas.configure(scrollregion=canvas.bbox('all')))
        canvas.create_window((0,0), window=scroll_frame, anchor='nw')
        canvas.configure(yscrollcommand=scrollbar.set)
        canvas.pack(side='left', fill='both', expand=True)
        scrollbar.pack(side='right', fill='y')
        for i, (action, keys) in enumerate(shortcuts):
            tk.Label(scroll_frame, text=action, bg=BG_COLOR, fg=FG_COLOR, font=FONT, anchor='w').grid(row=i, column=0, sticky='w', pady=2, padx=4)
            tk.Label(scroll_frame, text=keys, bg=BG_COLOR, fg=ACCENT_COLOR, font=FONT_BOLD, anchor='e').grid(row=i, column=1, sticky='e', pady=2, padx=4)
        tk.Button(top, text='Close', command=top.destroy, bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT, relief='flat').pack(pady=12)

    def add_to_startup(self):
        import os, sys
        import shutil
        import getpass
        import tkinter.messagebox as messagebox
        # Windows only: add shortcut to Startup folder
        if sys.platform != 'win32':
            messagebox.showinfo('Auto Startup', 'Auto startup is only supported on Windows.')
            return
        startup_dir = os.path.join(os.environ['APPDATA'], r'Microsoft\Windows\Start Menu\Programs\Startup')
        exe_path = sys.executable if getattr(sys, 'frozen', False) else sys.argv[0]
        shortcut_path = os.path.join(startup_dir, 'PDFReader.lnk')
        try:
            import pythoncom
            from win32com.shell import shell, shellcon
            from win32com.client import Dispatch
            shell = Dispatch('WScript.Shell')
            shortcut = shell.CreateShortCut(shortcut_path)
            shortcut.Targetpath = exe_path
            shortcut.WorkingDirectory = os.path.dirname(exe_path)
            shortcut.IconLocation = os.path.abspath('icon.ico')
            shortcut.save()
            messagebox.showinfo('Auto Startup', 'PDF Reader will now start automatically with Windows.')
        except Exception as e:
            messagebox.showerror('Auto Startup', f'Failed to add to startup: {e}')

    def _on_close(self):
        if pystray is None:
            self.destroy()
            import sys
            sys.exit(0)
        else:
            self.withdraw()
            def show_window(icon, item):
                self.after(0, self._show_from_tray)
            def quit_app(icon, item):
                icon.stop()
                self.after(0, self._really_exit)
            image_path = 'icon.ico'
            try:
                icon_img = PILImage.open(image_path)
            except Exception:
                icon_img = PILImage.new('RGB', (64, 64), color='gray')
            menu = pystray.Menu(
                pystray.MenuItem('Show', show_window),
                pystray.MenuItem('Exit', quit_app)
            )
            self.tray_icon = pystray.Icon('PDFReader', icon_img, 'PDF Reader', menu)
            threading.Thread(target=self.tray_icon.run, daemon=True).start()
    def _show_from_tray(self):
        self.deiconify()
        if hasattr(self, 'tray_icon') and self.tray_icon:
            self.tray_icon.stop()
            self.tray_icon = None
    def _really_exit(self):
        self.destroy()
        import sys
        sys.exit(0)

    def _exit_form_mode(self):
        self.set_annotation_mode('none')

    def _delete_annot_under_mouse(self, event):
        if not self.pdf_doc:
            return
        page = self.pdf_doc.load_page(self.current_page)
        x = self.canvas.canvasx(self.winfo_pointerx() - self.canvas.winfo_rootx())
        y = self.canvas.canvasy(self.winfo_pointery() - self.canvas.winfo_rooty())
        found = False
        for annot in page.annots():
            rect = annot.rect
            if rect.contains(fitz.Point(*self.canvas_to_pdf(x, y))):
                self._push_undo('remove_annot', {'page': self.current_page, 'annots': [annot.xref]})
                page.delete_annot(annot)
                found = True
                break
        if found:
            self.pdf_doc.saveIncr()
            self.show_page()
            self.status.config(text='Annotation deleted (Delete key).')
        else:
            self.status.config(text='No annotation detected under mouse.')

    def _copy_annot(self, event):
        if not self.pdf_doc:
            return
        page = self.pdf_doc.load_page(self.current_page)
        x = self.canvas.canvasx(self.winfo_pointerx() - self.canvas.winfo_rootx())
        y = self.canvas.canvasy(self.winfo_pointery() - self.canvas.winfo_rooty())
        for annot in page.annots():
            rect = annot.rect
            if rect.contains(fitz.Point(*self.canvas_to_pdf(x, y))):
                if annot.type[0] ==  fitz.PDF_ANNOT_FREETEXT:
                    self._copied_annot_data = {
                        'type': 'text',
                        'text': annot.info.get('content', ''),
                        'rect': annot.rect,
                        'color': annot.colors['stroke']
                    }
                    self.status.config(text='Text annotation copied.')
                    return
        self.status.config(text='No annotation detected to copy.')

    def _paste_annot(self, event):
        if not self.pdf_doc or not self._copied_annot_data:
            self.status.config(text='Nothing to paste.')
            return
        page = self.pdf_doc.load_page(self.current_page)
        x = self.canvas.canvasx(self.winfo_pointerx() - self.canvas.winfo_rootx())
        y = self.canvas.canvasy(self.winfo_pointery() - self.canvas.winfo_rooty())
        if self._copied_annot_data['type'] == 'text':
            w = self._copied_annot_data['rect'].width
            h = self._copied_annot_data['rect'].height
            px, py = self.canvas_to_pdf(x, y)
            annot = page.add_freetext_annot(fitz.Rect(px, py, px + w, py + h), self._copied_annot_data['text'], fontsize=12, fontname="helv", align=0)
            annot.set_colors(stroke=self._copied_annot_data['color'], fill=None)
            annot.update()
            self._push_undo('add_annot', {'page': self.current_page, 'annots': [annot.xref]})
            self.pdf_doc.saveIncr()
            self.show_page()
            self.status.config(text='Text annotation pasted.')
        else:
            self.status.config(text='Only text annotation copy/paste supported.')

if __name__ == '__main__':
    app = PDFReaderApp()
    app.mainloop()
