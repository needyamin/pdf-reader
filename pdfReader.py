import tkinter as tk
from tkinter import ttk, filedialog, messagebox, simpledialog
from PIL import Image, ImageTk
import fitz  # PyMuPDF
import os
import sys
import threading
import json
import requests
import datetime
import webbrowser
import logging
from pathlib import Path
import socket
import time

def resource_path(relative_path):
    """Return absolute path to resource, works for dev and Nuitka onefile"""
    if hasattr(sys, '_MEIPASS'):
        base_path = sys._MEIPASS
    else:
        base_path = os.path.abspath(".")
    return os.path.join(base_path, relative_path)

def get_user_data_dir():
    """Return per-user writable data directory for the application.

    On Windows, prefers %APPDATA% (Roaming). Falls back to the user's home directory if unavailable.
    """
    if sys.platform == 'win32':
        base_dir = os.environ.get('APPDATA') or os.path.expanduser('~')
    else:
        base_dir = os.path.expanduser('~')
    return os.path.join(base_dir, 'Advanced PDF Reader')

def setup_logging():
    """Setup minimal logging for production"""
    # Only log critical errors in production
    logging.basicConfig(
        level=logging.ERROR,
        format='%(levelname)s: %(message)s',
        handlers=[logging.StreamHandler(sys.stderr)]
    )
    
    # Suppress all library logging
    logging.getLogger('PIL').setLevel(logging.CRITICAL)
    logging.getLogger('fitz').setLevel(logging.CRITICAL)
    logging.getLogger('requests').setLevel(logging.CRITICAL)
    logging.getLogger('urllib3').setLevel(logging.CRITICAL)
    
    return logging.getLogger(__name__)

# Initialize logger
logger = setup_logging()

# Single instance lock
SINGLE_INSTANCE_PORT = 12345
_instance_socket = None

def is_already_running():
    """Check if another instance is already running"""
    global _instance_socket
    try:
        _instance_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        _instance_socket.bind(('localhost', SINGLE_INSTANCE_PORT))
        _instance_socket.listen(1)
        return False  # We're the first instance
    except OSError:
        # Port is already in use, another instance is running
        return True

def cleanup_single_instance():
    """Clean up single instance socket"""
    global _instance_socket
    if _instance_socket:
        try:
            _instance_socket.close()
        except:
            pass
        _instance_socket = None

# ✅ Asset paths (read-only, bundled inside onefile)
ASSET_DIR = resource_path('assets/images/YAMiN_HOSSAIN.png')
ICON_PATH = resource_path('assets/icons/icon.ico')
LOADING_IMG_PATH = resource_path('assets/images/loading.png')
LICENSE_FILE = resource_path('assets/json/license_info.json')

# ✅ Writable paths (stored in user profile, no external assets folder needed)
USER_DATA_DIR = get_user_data_dir()
SESSION_FILE = os.path.join(USER_DATA_DIR, 'last_session.json')
TOKEN_FILE = os.path.join(USER_DATA_DIR, 'github_token.json')

# # Asset folder structure
# ASSET_DIR = os.path.join(os.path.dirname(__file__), 'assets')
# ICON_PATH = os.path.join(ASSET_DIR, 'icons', 'icon.ico')
# LOADING_IMG_PATH = os.path.join(ASSET_DIR, 'images', 'loading.png')
# SESSION_FILE = os.path.join(ASSET_DIR, 'json', 'last_session.json')
# TOKEN_FILE = os.path.join(ASSET_DIR, 'json', 'github_token.json')
# LICENSE_FILE = os.path.join(ASSET_DIR, 'json', 'license_info.json')

try:
    import pystray
    from PIL import Image as PILImage
except ImportError:
    pystray = None
if sys.platform == 'win32':
    try:
        from ctypes import windll
        windll.shcore.SetProcessDpiAwareness(1)
        # Set app user model ID for proper taskbar grouping
        try:
            windll.shell32.SetCurrentProcessExplicitAppUserModelID("PDFReader.YAMiN.2.0.0")
        except Exception:
            pass
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
FONT_SMALL = ('Segoe UI', 9)

__version__ = "2.0.0"
GITHUB_REPO = "needyamin/pdf-reader"

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
        
        # Single instance check
        if is_already_running():
            # Another instance is already running
            messagebox.showinfo("PDF Reader", "PDF Reader is already running.")
            sys.exit(0)
        self.title('Advanced PDF Reader')
        self.geometry('1200x800')
        self.configure(bg=BG_COLOR)
        self._fullscreen = False  # Track fullscreen state
        self._current_pdf_path = None  # Track current file path for Save
        # Set application icon (window, taskbar, startbar)
        try:
            self.iconbitmap(ICON_PATH)
            # Also set the window icon for taskbar
            self.wm_iconbitmap(ICON_PATH)
            # Windows-specific taskbar icon
            if sys.platform == 'win32':
                try:
                    import win32gui
                    import win32con
                    # Get the window handle
                    hwnd = self.winfo_id()
                    # Load the icon
                    icon_handle = windll.user32.LoadImageW(
                        None, ICON_PATH, win32con.IMAGE_ICON, 0, 0,
                        win32con.LR_LOADFROMFILE | win32con.LR_DEFAULTSIZE
                    )
                    if icon_handle:
                        # Set the icon for the window
                        windll.user32.SendMessageW(hwnd, win32con.WM_SETICON, win32con.ICON_BIG, icon_handle)
                        windll.user32.SendMessageW(hwnd, win32con.WM_SETICON, win32con.ICON_SMALL, icon_handle)
                except Exception as e:
                    pass  # Icon setting failed, continue without it
        except Exception as e:
            pass  # Window icon failed, continue without it
            # Create a default icon if the file doesn't exist
            try:
                from PIL import ImageTk
                # Create a simple default icon
                img = PILImage.new('RGB', (32, 32), color='blue')
                self.iconphoto(True, ImageTk.PhotoImage(img))
            except Exception:
                pass
        # Ensure app fully exits on close (no background/system tray)
        self.protocol('WM_DELETE_WINDOW', self._on_close)
        self.bind('<<ReallyExit>>', lambda e: self._really_exit())
        
        # Clean up single instance socket on close
        self.bind('<Destroy>', lambda e: cleanup_single_instance())
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
        # Save PDF (always ask for location)
        self.bind_all('<Control-s>', lambda e: self.save_pdf_as())
        # Save As (same as Save)
        self.bind_all('<Control-Shift-S>', lambda e: self.save_pdf_as())
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
        self.bind_all('<Escape>', lambda e: self.toggle_sidebar())
        # Close
        self.bind_all('<Control-q>', lambda e: self.quit())
        # self.bind_all('<Escape>', lambda e: self.quit())  # ESC now toggles sidebar
        self.bind_all('<Return>', lambda e: self._goto_page_from_entry())
        self.focus_set()
        # Undo/redo stacks removed as requested
        self.bind_all('<Delete>', lambda e: self._delete_annot_under_mouse(e))
        # Copied annotation data removed as requested
        self._loading_label = None  # For sidebar loading indicator
        self._session_loaded = False
        # Check if a PDF was passed as command-line argument
        self._pdf_from_cmdline = len(sys.argv) > 1 and sys.argv[1].lower().endswith('.pdf') and os.path.exists(sys.argv[1])
        # Only load session if no PDF was passed as command-line argument
        if not self._pdf_from_cmdline:
            self.after(200, self._load_last_session)
        # Auto-update check on startup
        self.after(1000, self.auto_update)
        # Add F11 for fullscreen toggle
        self.bind_all('<F11>', lambda e: self.toggle_fullscreen())
        self.bind_all('<Escape>', lambda e: self.exit_fullscreen())
        self._check_license()  # <-- moved here, after window is created

    def _setup_style(self):
        style = ttk.Style(self)
        style.theme_use('clam')
        
        # Configure modern dark theme
        style.configure('TFrame', background=BG_COLOR)
        style.configure('TLabel', background=BG_COLOR, foreground=FG_COLOR, font=FONT)
        style.configure('TButton', 
                       background=TOOLBAR_COLOR, 
                       foreground=FG_COLOR, 
                       borderwidth=0, 
                       focusthickness=0, 
                       focuscolor=TOOLBAR_COLOR, 
                       font=FONT,
                       padding=(8, 4))
        style.map('TButton', 
                 background=[('active', BUTTON_ACTIVE), ('pressed', ACCENT_COLOR)],
                 foreground=[('active', FG_COLOR), ('pressed', BG_COLOR)])
        style.configure('Hover.TButton', background=BUTTON_ACTIVE, foreground=ACCENT_COLOR)
        
        # Configure scrollbars
        style.configure('TScrollbar', 
                       background=SIDEBAR_COLOR, 
                       troughcolor=TOOLBAR_COLOR,
                       borderwidth=0,
                       arrowcolor=FG_COLOR)
        style.map('TScrollbar', 
                 background=[('active', ACCENT_COLOR)])
        
        # Configure entry widgets
        style.configure('TEntry',
                       fieldbackground=TOOLBAR_COLOR,
                       foreground=FG_COLOR,
                       insertcolor=FG_COLOR,
                       borderwidth=1,
                       relief='solid')
        
        # Configure checkbuttons and radiobuttons
        style.configure('TCheckbutton',
                       background=BG_COLOR,
                       foreground=FG_COLOR,
                       focuscolor=ACCENT_COLOR)
        style.configure('TRadiobutton',
                       background=BG_COLOR,
                       foreground=FG_COLOR,
                       focuscolor=ACCENT_COLOR)

    def _create_widgets(self):
        # Menu bar
        self.menu = tk.Menu(self)
        self.config(menu=self.menu)
        # File menu
        file_menu = tk.Menu(self.menu, tearoff=0)
        file_menu.add_command(label='Open...', command=self.open_pdf, accelerator='Ctrl+O')
        file_menu.add_command(label='Save', command=self.save_pdf_as, accelerator='Ctrl+S')
        file_menu.add_command(label='Save As...', command=self.save_pdf_as, accelerator='Ctrl+Shift+S')
        file_menu.add_separator()
        file_menu.add_command(label='Export as Image...', command=self.export_image)
        file_menu.add_separator()
        file_menu.add_command(label='Check for Updates', command=lambda: self.auto_update(manual=True))
        file_menu.add_separator()
        file_menu.add_command(label='Exit', command=self.exit_app, accelerator='Ctrl+Q')
        self.menu.add_cascade(label='File', menu=file_menu)
        # Edit menu removed as requested
        # View menu
        view_menu = tk.Menu(self.menu, tearoff=0)
        view_menu.add_command(label='Zoom In', command=lambda: self.change_zoom(1.25), accelerator='Ctrl++')
        view_menu.add_command(label='Zoom Out', command=lambda: self.change_zoom(0.8), accelerator='Ctrl+-')
        view_menu.add_command(label='Fit to Window', command=self.toggle_fit, accelerator='F')
        view_menu.add_separator()
        view_menu.add_command(label='Full Screen', command=self.toggle_fullscreen, accelerator='F11')
        view_menu.add_separator()
        view_menu.add_command(label='Toggle Sidebar', command=self.toggle_sidebar, accelerator='S')
        self.menu.add_cascade(label='View', menu=view_menu)
        # Annotate menu (hidden for now)
        # annotate_menu = tk.Menu(self.menu, tearoff=0)
        # annotate_menu.add_command(label='Highlight', command=lambda: self.set_annotation_mode('highlight'))
        # annotate_menu.add_command(label='Draw', command=lambda: self.set_annotation_mode('draw'))
        # annotate_menu.add_command(label='Text Note', command=lambda: self.set_annotation_mode('text'))
        # annotate_menu.add_command(label='Add Image', command=lambda: self.set_annotation_mode('image'))
        # annotate_menu.add_command(label='Fill Form', command=lambda: self.set_annotation_mode('form'))
        # annotate_menu.add_separator()
        # annotate_menu.add_command(label='Eraser 🧹', command=lambda: self.set_annotation_mode('eraser'))
        # self.menu.add_cascade(label='Annotate', menu=annotate_menu)
        # Help menu
        help_menu = tk.Menu(self.menu, tearoff=0)
        help_menu.add_command(label='About', command=self.show_about)
        help_menu.add_command(label='Shortcuts', command=self.show_shortcuts)
        help_menu.add_separator()
        self.menu.add_cascade(label='Help', menu=help_menu)
        # Activate Software
        activate_software = tk.Menu(self.menu, tearoff=0)
        activate_software.add_command(label='Active', command=self._prompt_license)
        activate_software.add_separator()
        activate_software.add_command(label='Buy Me Coffee', command=self.show_license)
        self.menu.add_cascade(label='Active', menu=activate_software)
        

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
        # Configure for smooth scrolling
        self.thumbnail_canvas.configure(yscrollincrement=1)
        self.thumbnail_frame = tk.Frame(self.thumbnail_canvas, bg=SIDEBAR_COLOR)
        self.thumbnail_canvas.create_window((0, 0), window=self.thumbnail_frame, anchor='nw')
        self.thumbnail_frame.bind('<Configure>', lambda e: self.thumbnail_canvas.configure(scrollregion=self.thumbnail_canvas.bbox('all')))
        # Sidebar scrolling and keyboard navigation
        # Bind mouse wheel to both canvas and frame for reliable sidebar scrolling
        self.thumbnail_canvas.bind('<MouseWheel>', self._on_sidebar_mousewheel)
        self.thumbnail_canvas.bind('<Button-4>', self._on_sidebar_mousewheel)  # Linux scroll up
        self.thumbnail_canvas.bind('<Button-5>', self._on_sidebar_mousewheel)  # Linux scroll down
        self.thumbnail_frame.bind('<MouseWheel>', self._on_sidebar_mousewheel)
        self.thumbnail_frame.bind('<Button-4>', self._on_sidebar_mousewheel)
        self.thumbnail_frame.bind('<Button-5>', self._on_sidebar_mousewheel)
        self._propagate_mousewheel_to_canvas(self.thumbnail_frame, self.thumbnail_canvas)
        # Sidebar keyboard navigation (only when sidebar has focus)
        self.thumbnail_canvas.bind('<Up>', lambda e: self._sidebar_select_relative(-1))
        self.thumbnail_canvas.bind('<Down>', lambda e: self._sidebar_select_relative(1))
        self.thumbnail_canvas.bind('<Prior>', lambda e: self._sidebar_select_relative(-5))  # PageUp
        self.thumbnail_canvas.bind('<Next>', lambda e: self._sidebar_select_relative(5))   # PageDown
        self.thumbnail_canvas.bind('<Home>', lambda e: self.go_to_page(0))
        self.thumbnail_canvas.bind('<End>', lambda e: self.go_to_page(len(self.pdf_doc)-1 if self.pdf_doc else 0))
        self.thumbnail_canvas.bind('<Return>', lambda e: self.go_to_page(self.current_page))

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

        # Status bar with improved styling
        self.status = tk.Label(self, 
                              text='No document loaded', 
                              bg=STATUS_COLOR, 
                              fg=FG_COLOR, 
                              anchor='w', 
                              font=FONT,
                              relief='flat',
                              bd=1,
                              highlightthickness=0)
        self.status.pack(side='bottom', fill='x', padx=0, pady=0)

        # Mouse zoom (Ctrl+Wheel)
        self.canvas.bind('<Control-MouseWheel>', self._on_canvas_ctrl_mousewheel)
        self.canvas.bind('<Control-Button-4>', self._on_canvas_ctrl_mousewheel)  # Linux scroll up
        self.canvas.bind('<Control-Button-5>', self._on_canvas_ctrl_mousewheel)  # Linux scroll down
        # Modern Annotation Toolbar
        self._create_annotation_toolbar()

        # Annotation options (color, width, opacity)
        self._create_annotation_options()

        # Mouse wheel scroll for main PDF canvas
        self.canvas.bind('<MouseWheel>', self._on_canvas_mousewheel)         # Windows
        self.canvas.bind('<Button-4>', self._on_canvas_mousewheel)           # Linux scroll up
        self.canvas.bind('<Button-5>', self._on_canvas_mousewheel)           # Linux scroll down
        
        # Keyboard scrolling for main PDF canvas
        # Up/Down arrows for vertical scrolling
        self.canvas.bind('<Up>', lambda e: self._on_canvas_key_scroll('up'))
        self.canvas.bind('<Down>', lambda e: self._on_canvas_key_scroll('down'))
        # Ctrl+Left/Right for horizontal scrolling (to avoid conflict with page navigation)
        self.canvas.bind('<Control-Left>', lambda e: self._on_canvas_key_scroll('left'))
        self.canvas.bind('<Control-Right>', lambda e: self._on_canvas_key_scroll('right'))
        # Shift + arrow keys for faster scrolling
        self.canvas.bind('<Shift-Up>', lambda e: self._on_canvas_key_scroll('up', 50))
        self.canvas.bind('<Shift-Down>', lambda e: self._on_canvas_key_scroll('down', 50))
        self.canvas.bind('<Shift-Control-Left>', lambda e: self._on_canvas_key_scroll('left', 50))
        self.canvas.bind('<Shift-Control-Right>', lambda e: self._on_canvas_key_scroll('right', 50))
        # Also bind to display frame for better coverage
        self.display_frame.bind('<Up>', lambda e: self._on_canvas_key_scroll('up'))
        self.display_frame.bind('<Down>', lambda e: self._on_canvas_key_scroll('down'))
        self.display_frame.bind('<Control-Left>', lambda e: self._on_canvas_key_scroll('left'))
        self.display_frame.bind('<Control-Right>', lambda e: self._on_canvas_key_scroll('right'))
        self.display_frame.bind('<Shift-Up>', lambda e: self._on_canvas_key_scroll('up', 50))
        self.display_frame.bind('<Shift-Down>', lambda e: self._on_canvas_key_scroll('down', 50))
        self.display_frame.bind('<Shift-Control-Left>', lambda e: self._on_canvas_key_scroll('left', 50))
        self.display_frame.bind('<Shift-Control-Right>', lambda e: self._on_canvas_key_scroll('right', 50))
        
        # Set focus to main canvas by default so arrow keys work for scrolling
        self.canvas.focus_set()
        
        # Simplified focus management for better performance
        self._page_entry_has_focus = False
        
        def ensure_canvas_focus(event=None):
            if not self._page_entry_has_focus:
                self.canvas.focus_set()
        
        # Minimal focus management - only essential bindings
        self.canvas.bind('<FocusOut>', lambda e: self.after(100, ensure_canvas_focus))

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
        make_btn('Save', self.save_pdf_as, '💾', 'Save PDF (Ctrl+S)')
        make_btn('Prev', self.prev_page, '⬅️', 'Previous Page (Left, PageUp, Shift+Space)')
        make_btn('Next', self.next_page, '➡️', 'Next Page (Right, PageDown, Space)')
        make_btn('Zoom +', lambda: self.change_zoom(1.25), '➕', 'Zoom In (Ctrl +, +, =)')
        make_btn('Zoom -', lambda:
                  self.change_zoom(0.8), '➖', 'Zoom Out (Ctrl -, -)')
        make_btn('Rotate', self.rotate_page, '🔄', 'Rotate Page (R)')
        # REMOVE the Annotate placeholder button
        # make_btn('Annotate', self.annotate_placeholder, '🖊️', 'Annotation tools (coming soon)')
        # Fit to Window toggle
        self.fit_btn = ttk.Checkbutton(self.toolbar, text='🖥️ Fit to Window', variable=self.fit_to_window, command=self.show_page, style='TButton')
        self.fit_btn.pack(side='left', padx=(16,2), pady=8)
        Tooltip(self.fit_btn, 'Toggle Fit to Window (F, Ctrl+F)')
        # Page number entry with improved styling
        # Page navigation input with improved UX
        page_frame = tk.Frame(self.toolbar, bg=TOOLBAR_COLOR)
        page_frame.pack(side='left', padx=(16,12), pady=8)
        
        # Page label
        page_label = tk.Label(page_frame, text='Page:', bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT)
        page_label.pack(side='left')
        
        self.page_entry_var = tk.StringVar()
        self.page_entry = tk.Entry(page_frame, 
                                  width=6, 
                                  textvariable=self.page_entry_var, 
                                  font=FONT, 
                                  bg='#2d3142', 
                                  fg=FG_COLOR, 
                                  insertbackground=FG_COLOR, 
                                  relief='solid', 
                                  bd=1,
                                  justify='center',
                                  highlightthickness=2,
                                  highlightbackground=ACCENT_COLOR,
                                  highlightcolor=ACCENT_COLOR)
        self.page_entry.pack(side='left', padx=(4,2))
        
        # Total pages label
        self.page_total_label = tk.Label(page_frame, 
                                        text='/ 0', 
                                        bg=TOOLBAR_COLOR, 
                                        fg=FG_COLOR, 
                                        font=FONT)
        self.page_total_label.pack(side='left')
        
        # Go button for better UX
        go_btn = tk.Button(page_frame, 
                          text='Go', 
                          command=self._goto_page_from_entry,
                          bg=ACCENT_COLOR, 
                          fg='white', 
                          font=FONT, 
                          relief='flat', 
                          bd=0,
                          padx=8,
                          cursor='hand2')
        go_btn.pack(side='left', padx=(4,0))
        
        # Enhanced tooltip and bindings
        Tooltip(self.page_entry, 'Enter page number and press Enter or click Go')
        Tooltip(go_btn, 'Go to specified page')
        
        # Minimal page entry bindings for performance
        self.page_entry.bind('<FocusIn>', lambda e: setattr(self, '_page_entry_has_focus', True))
        self.page_entry.bind('<FocusOut>', lambda e: setattr(self, '_page_entry_has_focus', False))
        self.page_entry.bind('<Return>', lambda e: self._goto_page_from_entry())
        self.page_entry.bind('<Escape>', lambda e: self.canvas.focus_set())
        # Remove the problematic KeyPress binding that was blocking input
        # self.page_entry.bind('<KeyPress>', self._validate_page_input)
        
        # Don't trace the variable as it interferes with typing
        # self.page_entry_var.trace('w', self._update_page_entry)

    def _show_sidebar_loading(self, show=True):
        if show:
            if not hasattr(self, '_loading_label') or not self._loading_label:
                if hasattr(self, 'thumbnail_frame'):
                    self._loading_label = tk.Label(self.thumbnail_frame, text='Loading...', bg=SIDEBAR_COLOR, fg=ACCENT_COLOR, font=FONT_BOLD)
                    self._loading_label.pack(pady=20)
        else:
            if hasattr(self, '_loading_label') and self._loading_label:
                self._loading_label.destroy()
                self._loading_label = None

    def open_pdf(self, file_path=None):
        # Opening PDF
        if not file_path:
            file_path = filedialog.askopenfilename(filetypes=[('PDF Files', '*.pdf')])
        if not file_path:
            return
        self._show_sidebar_loading(True)
        # Removed update_idletasks for performance
        try:
            self.pdf_doc = fitz.open(file_path)
            self._current_pdf_path = file_path  # Track current file path
            # PDF loaded successfully
            self.current_page = 0
            self.zoom = 1.0
            self.rotation = 0
            self.status.config(text=f'Loaded: {os.path.basename(file_path)}')
            self._render_thumbnails()
            self.fit_to_window.set(True)  # Always fit to window on open
            self.show_page()
            self.after(100, self.show_page)  # Ensure fit after canvas is ready
            # Save session after successful PDF load
            self._save_last_session(file_path, self.current_page)
        except fitz.FileNotFoundError:
            messagebox.showerror('File Not Found', f'The PDF file was not found:\n{file_path}')
        except fitz.EmptyFileError:
            messagebox.showerror('Invalid PDF', f'The PDF file is empty or corrupted:\n{file_path}')
        except fitz.FileDataError:
            messagebox.showerror('PDF Error', f'The PDF file has invalid data:\n{file_path}')
        except PermissionError:
            messagebox.showerror('Permission Denied', f'Permission denied accessing the PDF file:\n{file_path}')
        except Exception as e:
            messagebox.showerror('Error', f'An unexpected error occurred while opening the PDF:\n{str(e)}')
        self._show_sidebar_loading(False)

    def _render_thumbnails(self):
        for widget in self.thumbnail_frame.winfo_children():
            widget.destroy()
        self.thumbnails = []
        
        # Standard thumbnail size to prevent overlap
        max_thumb_width = 120
        max_thumb_height = 160
        
        for i in range(len(self.pdf_doc)):
            page = self.pdf_doc.load_page(i)
            pix = page.get_pixmap(matrix=fitz.Matrix(0.18, 0.18))
            img = Image.frombytes('RGB', [pix.width, pix.height], pix.samples)
            
            # Resize image to standard thumbnail size while maintaining aspect ratio
            img.thumbnail((max_thumb_width, max_thumb_height), Image.Resampling.LANCZOS)
            thumb = ImageTk.PhotoImage(img)
            self.thumbnails.append(thumb)
            
            # Create a frame to hold thumbnail with fixed positioning
            thumb_frame = tk.Frame(self.thumbnail_frame, bg=SIDEBAR_COLOR, bd=2, relief='solid', 
                                 highlightthickness=2, width=max_thumb_width, height=max_thumb_height)
            thumb_frame.pack(pady=4, padx=6, fill='x')
            thumb_frame.pack_propagate(False)  # Prevent frame from shrinking
            
            # Center the thumbnail image in the frame
            btn = tk.Label(thumb_frame, image=thumb, bg=SIDEBAR_COLOR, bd=0, highlightthickness=0)
            btn.place(relx=0.5, rely=0.5, anchor='center')
            
            # Page number badge (positioned on top-right corner)
            page_num_label = tk.Label(thumb_frame, text=str(i+1), bg=ACCENT_COLOR, fg=FG_COLOR, 
                                    font=('Segoe UI', 7, 'bold'), anchor='center',
                                    relief='raised', bd=1)
            # Position the badge in the top-right corner
            page_num_label.place(relx=0.85, rely=0.05, width=20, height=16)
            
            # Bind events to the frame
            thumb_frame.bind('<Button-1>', lambda e, i=i: self.go_to_page(i))
            thumb_frame.bind('<Enter>', lambda e, f=thumb_frame: f.configure(bg=BUTTON_ACTIVE))
            thumb_frame.bind('<Leave>', lambda e, f=thumb_frame, idx=i: self._update_thumbnail_highlight(f, idx))
            
            # Also bind to the image and label for better UX
            btn.bind('<Button-1>', lambda e, i=i: self.go_to_page(i))
            btn.bind('<Enter>', lambda e, f=thumb_frame: f.configure(bg=BUTTON_ACTIVE))
            btn.bind('<Leave>', lambda e, f=thumb_frame, idx=i: self._update_thumbnail_highlight(f, idx))
            
            page_num_label.bind('<Button-1>', lambda e, i=i: self.go_to_page(i))
            page_num_label.bind('<Enter>', lambda e, f=thumb_frame: f.configure(bg=BUTTON_ACTIVE))
            page_num_label.bind('<Leave>', lambda e, f=thumb_frame, idx=i: self._update_thumbnail_highlight(f, idx))
            
            # Highlight current page
            self._update_thumbnail_highlight(thumb_frame, i)
            self._propagate_mousewheel_to_canvas(thumb_frame, self.thumbnail_canvas)
        
        # Force update of scroll region after all thumbnails are created
        # Removed update_idletasks for performance
        self.thumbnail_canvas.configure(scrollregion=self.thumbnail_canvas.bbox('all'))
        
        # Enable smooth scrolling with proper configuration
        self.thumbnail_canvas.configure(yscrollincrement=1)

    def _update_thumbnail_highlight(self, btn, idx):
        if idx == self.current_page:
            btn.configure(bg=HIGHLIGHT_COLOR, bd=3, relief='solid', highlightbackground=ACCENT_COLOR, highlightcolor=ACCENT_COLOR)
            # Update page number badge color for current page
            for child in btn.winfo_children():
                if isinstance(child, tk.Label) and child.cget('text').isdigit():
                    child.configure(bg=FG_COLOR, fg=HIGHLIGHT_COLOR)
        else:
            btn.configure(bg=SIDEBAR_COLOR, bd=2, relief='solid', highlightbackground=SIDEBAR_COLOR, highlightcolor=SIDEBAR_COLOR)
            # Update page number badge color for other pages
            for child in btn.winfo_children():
                if isinstance(child, tk.Label) and child.cget('text').isdigit():
                    child.configure(bg=ACCENT_COLOR, fg=FG_COLOR)

    def show_page(self):
        if not self.pdf_doc:
            self.page_entry_var.set('')
            self.page_total_label.config(text='/ 0')
            return
        
        # Check if canvas is ready
        canvas_width = self.canvas.winfo_width()
        canvas_height = self.canvas.winfo_height()
        if canvas_width < 10 or canvas_height < 10:
            return
        
        # Store current scroll position before page change (optimized)
        try:
            current_scroll_x = self.canvas.canvasx(0)
            current_scroll_y = self.canvas.canvasy(0)
        except:
            current_scroll_x = current_scroll_y = 0
        # Always reload the page from the PDF
        page = self.pdf_doc.load_page(self.current_page)
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
            self.canvas.delete('pdf_image')
            self.canvas.delete('annotation')
            self.canvas.create_image(canvas_width//2, canvas_height//2, image=self.images[0], anchor='center', tags='pdf_image')
            self.canvas.config(xscrollcommand=self.h_scroll.set, yscrollcommand=self.v_scroll.set)
            self.h_scroll.grid_remove()
            self.v_scroll.grid_remove()
        else:
            self._pdf_scale = zoom
            # Calculate offsets for centering
            self._pdf_offset_x = (canvas_width - img.width) // 2
            self._pdf_offset_y = (canvas_height - img.height) // 2
            self._pdf_render_width = img.width
            self._pdf_render_height = img.height
            self.images = [ImageTk.PhotoImage(img)]
            
            # Set scrollregion to include PDF content plus centering space
            # This allows the PDF to be centered in the canvas
            scroll_width = max(img.width, canvas_width)
            scroll_height = max(img.height, canvas_height)
            
            # Add padding to center the PDF
            if img.width < canvas_width:
                scroll_width = canvas_width
            if img.height < canvas_height:
                scroll_height = canvas_height
            
            self.canvas.config(scrollregion=(0, 0, scroll_width, scroll_height))
            self.canvas.delete('pdf_image')
            self.canvas.delete('annotation')
            
            # Position image at center of scroll region
            center_x = scroll_width // 2
            center_y = scroll_height // 2
            self.canvas.create_image(center_x, center_y, image=self.images[0], anchor='center', tags='pdf_image')
            
            self.canvas.xview_moveto(0.0)
            self.canvas.yview_moveto(0.0)
            
            self.h_scroll.grid()
            self.v_scroll.grid()
        # Render annotations on the canvas
        self._render_annotations()
        
        # Reset scroll position tracking for new pages
        self._last_scroll_x = 0
        self._last_scroll_y = 0
        
        self.page_entry_var.set(str(self.current_page+1))
        self.page_total_label.config(text=f'/ {len(self.pdf_doc)}')
        fname = self.pdf_doc.name if hasattr(self.pdf_doc, 'name') else ''
        # Get current focus widget for debugging
        focused_widget = self.focus_get()
        focus_info = f"Focus: {type(focused_widget).__name__}" if focused_widget else "Focus: None"
        self.status.config(text=f'{os.path.basename(fname)} | Page {self.current_page+1}/{len(self.pdf_doc)} | Zoom: {int(self.zoom*100)}% | Rotation: {self.rotation}° | Fit: {"ON" if self.fit_to_window.get() else "OFF"} | {focus_info}')
        
        # Update page entry to reflect current page (only if not focused to avoid interference)
        if hasattr(self, 'page_entry_var') and self.pdf_doc and not self._page_entry_has_focus:
            self.page_entry_var.set(str(self.current_page + 1))
        
        for idx, widget in enumerate(self.thumbnail_frame.winfo_children()):
            self._update_thumbnail_highlight(widget, idx)

    def prev_page(self):
        if self.pdf_doc and self.current_page > 0:
            self.current_page -= 1
            self.show_page()
            # Auto-scroll thumbnail to current page
            self.after(50, lambda: self._scroll_to_thumbnail(self.current_page))
            # Save session after page change
            if self.pdf_doc:
                self._save_last_session(self.pdf_doc.name, self.current_page)

    def next_page(self):
        if self.pdf_doc and self.current_page < len(self.pdf_doc) - 1:
            self.current_page += 1
            self.show_page()
            # Auto-scroll thumbnail to current page
            self.after(50, lambda: self._scroll_to_thumbnail(self.current_page))
            # Save session after page change
            if self.pdf_doc:
                self._save_last_session(self.pdf_doc.name, self.current_page)

    def go_to_page(self, page_num):
        if self.pdf_doc and 0 <= page_num < len(self.pdf_doc):
            self.current_page = page_num
            self.show_page()
            # Auto-scroll thumbnail to current page
            self.after(50, lambda: self._scroll_to_thumbnail(self.current_page))
            self._save_last_session(self.pdf_doc.name, self.current_page)

    def _validate_page_input(self, event):
        """Validate page input to only allow numbers and common keys"""
        char = event.char
        # Allow digits, backspace, delete, arrow keys, etc.
        if char and not (char.isdigit() or char in '\b\x7f'):
            return 'break'  # Prevent non-digit characters
        return None
    
    def _update_page_entry(self, *args):
        """Update page entry to show current page"""
        if self.pdf_doc:
            current_page_num = self.current_page + 1
            if self.page_entry_var.get() != str(current_page_num):
                self.page_entry_var.set(str(current_page_num))
    
    def _goto_page_from_entry(self):
        if not self.pdf_doc:
            return
        
        try:
            page_text = self.page_entry_var.get().strip()
            if not page_text:
                return
            
            page = int(page_text) - 1
            if 0 <= page < len(self.pdf_doc):
                self.current_page = page
                self.show_page()
                # Auto-scroll thumbnail to current page
                self.after(50, lambda: self._scroll_to_thumbnail(self.current_page))
                self._save_last_session(self.pdf_doc.name, self.current_page)
                
                # Visual feedback
                self.page_entry.configure(bg=ACCENT_COLOR)
                self.after(200, lambda: self.page_entry.configure(bg='#2d3142'))
                
                # Return focus to canvas after navigation
                self.after(100, lambda: self.canvas.focus_set())
            else:
                # Invalid page number - show error feedback
                self.page_entry.configure(bg='#ff6b6b')
                self.after(500, lambda: self.page_entry.configure(bg='#2d3142'))
                
        except ValueError:
            # Invalid input - show error feedback
            self.page_entry.configure(bg='#ff6b6b')
            self.after(500, lambda: self.page_entry.configure(bg='#2d3142'))

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
        # Debounce: only redraw after resizing stops for 200ms to reduce redraws
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
        self._page_entry_has_focus = True
        self.page_entry.focus_set()
        self.page_entry.select_range(0, 'end')

    def toggle_sidebar(self):
        if self.sidebar_visible:
            self.paned.forget(self.sidebar)
            self.sidebar_visible = False
        else:
            # Remove all panes
            for child in self.paned.panes():
                self.paned.forget(child)
            # Add sidebar first (left), then main display area (right)
            self.paned.add(self.sidebar, minsize=80)
            self.paned.add(self.display_outer, minsize=200)
            self.sidebar_visible = True
        self.show_page()

    def _sidebar_select_relative(self, direction):
        """Navigate through thumbnails using arrow keys"""
        if not self.pdf_doc:
            return
        new_page = self.current_page + direction
        if 0 <= new_page < len(self.pdf_doc):
            self.go_to_page(new_page)
            # Scroll to make the selected thumbnail visible
            self.after(100, lambda: self._scroll_to_thumbnail(new_page))
    
    def _scroll_to_thumbnail(self, page_num):
        """Scroll sidebar to make the specified thumbnail visible"""
        try:
            # Get the thumbnail widget for the specified page
            thumbnails = [w for w in self.thumbnail_frame.winfo_children() if isinstance(w, tk.Frame)]
            if 0 <= page_num < len(thumbnails):
                thumb = thumbnails[page_num]
                
                # Get thumbnail position and size
                thumb_y = thumb.winfo_y()
                thumb_height = thumb.winfo_height()
                canvas_height = self.thumbnail_canvas.winfo_height()
                frame_height = self.thumbnail_frame.winfo_height()
                
                # Calculate if thumbnail is already visible
                thumb_top = thumb_y
                thumb_bottom = thumb_y + thumb_height
                canvas_top = self.thumbnail_canvas.canvasy(0)
                canvas_bottom = canvas_top + canvas_height
                
                # Only scroll if thumbnail is not fully visible
                if thumb_top < canvas_top or thumb_bottom > canvas_bottom:
                    # Calculate scroll position to center the thumbnail
                    scroll_pos = (thumb_y - canvas_height/2 + thumb_height/2) / max(1, frame_height - canvas_height)
                    scroll_pos = max(0, min(1, scroll_pos))  # Clamp between 0 and 1
                    
                    # Smooth scroll to the position
                    self.thumbnail_canvas.yview_moveto(scroll_pos)
        except Exception:
            pass

    def _on_sidebar_mousewheel(self, event):
        # Smooth sidebar scrolling with adaptive speed
        canvas_height = self.thumbnail_canvas.winfo_height()
        frame_height = self.thumbnail_frame.winfo_height()
        
        # Calculate smooth scroll amount based on canvas size
        if canvas_height > 0:
            scroll_units = max(1, canvas_height // 20)  # 5% of canvas height
        else:
            scroll_units = 3  # Fallback for small canvases
        
        if hasattr(event, 'delta'):
            if event.delta > 0:
                self.thumbnail_canvas.yview_scroll(-scroll_units, 'units')
            elif event.delta < 0:
                self.thumbnail_canvas.yview_scroll(scroll_units, 'units')
        else:
            if event.num == 4:
                self.thumbnail_canvas.yview_scroll(-scroll_units, 'units')
            elif event.num == 5:
                self.thumbnail_canvas.yview_scroll(scroll_units, 'units')

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
            self._current_pdf_path = file_path  # Update current file path
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

    # Undo/redo methods removed as requested

    # Edit menu methods removed as requested

    def _create_annotation_toolbar(self):
        """Create a modern annotation toolbar with all tools."""
        self.annot_toolbar = tk.Frame(self.toolbar, bg=TOOLBAR_COLOR)
        self.annot_toolbar.pack(side='right', padx=8)
        
        # Separator
        separator = tk.Frame(self.annot_toolbar, width=1, bg=ACCENT_COLOR)
        separator.pack(side='left', fill='y', padx=4)
        
        self.annot_mode = tk.StringVar(value='none')
        self.annot_btns = {}
        
        # Annotation tools with modern icons and tooltips
        tools = [
            ('draw', '✏️', 'Free Draw', '#2196f3'),
            ('eraser', '🧹', 'Eraser', '#f44336')
        ]
        
        for mode, icon, tooltip, color in tools:
            btn = tk.Button(
                self.annot_toolbar,
                text=icon,
                font=('Segoe UI Emoji', 12),
                bg=TOOLBAR_COLOR,
                fg=FG_COLOR,
                activebackground=BUTTON_ACTIVE,
                activeforeground=FG_COLOR,
                relief='flat',
                bd=0,
                padx=8,
                pady=4,
                cursor='hand2',
                command=lambda m=mode: self.set_annotation_mode(m)
            )
            btn.pack(side='left', padx=1)
            
            # Add hover effects
            btn.bind('<Enter>', lambda e, b=btn, c=color: self._on_annot_btn_enter(e, b, c))
            btn.bind('<Leave>', lambda e, b=btn: self._on_annot_btn_leave(e, b))
            
            Tooltip(btn, tooltip)
            self.annot_btns[mode] = btn
        
    
    def _create_annotation_options(self):
        """Create annotation options (width only)."""
        self.annot_options = tk.Frame(self.toolbar, bg=TOOLBAR_COLOR)
        self.annot_options.pack(side='right', padx=8)
        
        # Color palette (4 primary colors)
        self.color_var = tk.StringVar(value='#ff0000')
        tk.Label(self.annot_options, text='Color:', bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT_SMALL).pack(side='left')
        
        # 4-color palette
        colors = ['#ff0000', '#0000ff', '#00ff00', '#000000']  # Red, Blue, Green, Black
        self.color_btns = []
        for color in colors:
            btn = tk.Button(
                self.annot_options,
                bg=color,
                width=2,
                height=1,
                relief='solid',
                bd=1,
                cursor='hand2',
                command=lambda c=color: self._set_color(c)
            )
            btn.pack(side='left', padx=1)
            self.color_btns.append(btn)
            if color == '#ff0000':  # Default selected color
                btn.config(relief='sunken', bd=2)
        
        # Width control
        tk.Label(self.annot_options, text='Width:', bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT_SMALL).pack(side='left')
        self.width_var = tk.IntVar(value=2)
        self.width_spin = tk.Spinbox(
            self.annot_options, 
            from_=1, 
            to=10, 
            width=3, 
            textvariable=self.width_var,
            font=FONT_SMALL,
            bg=TOOLBAR_COLOR,
            fg=FG_COLOR,
            insertbackground=FG_COLOR,
            relief='solid',
            bd=1
        )
        self.width_spin.pack(side='left', padx=2)
        Tooltip(self.width_spin, 'Set pen width (1-10)')
    
    def _set_color(self, color):
        """Set annotation color from palette."""
        self.color_var.set(color)
        # Update button states
        for btn in self.color_btns:
            btn.config(relief='solid', bd=1)
        # Highlight selected color
        for btn in self.color_btns:
            if btn.cget('bg') == color:
                btn.config(relief='sunken', bd=2)
                break
    
    def _on_annot_btn_enter(self, event, btn, color):
        """Handle annotation button hover enter."""
        btn.config(bg=color, fg='white')
    
    def _on_annot_btn_leave(self, event, btn):
        """Handle annotation button hover leave."""
        btn.config(bg=TOOLBAR_COLOR, fg=FG_COLOR)
    
    def _render_annotations(self):
        """Render PDF annotations on the canvas (highly optimized for performance)."""
        if not self.pdf_doc:
            return
        
        # Skip rendering if canvas is not ready (performance optimization)
        try:
            if self.canvas.winfo_width() < 10 or self.canvas.winfo_height() < 10:
                return
        except:
            return
        
        # Clear existing annotations first
        self.canvas.delete('annotation')
            
        try:
            page = self.pdf_doc.load_page(self.current_page)
            annotations = list(page.annots())  # Convert to list once for performance
            
            # Limit annotations to prevent lag on pages with many annotations
            if len(annotations) > 50:
                annotations = annotations[:50]  # Only render first 50 annotations
            
            for annot in annotations:
                rect = annot.rect
                x0, y0 = self.pdf_to_canvas(rect.x0, rect.y0)
                x1, y1 = self.pdf_to_canvas(rect.x1, rect.y1)
                
                # Skip if annotation is outside visible area (performance optimization)
                canvas_width = self.canvas.winfo_width()
                canvas_height = self.canvas.winfo_height()
                if x0 > canvas_width or x1 < 0 or y0 > canvas_height or y1 < 0:
                    continue
                
                if annot.type[0] == fitz.PDF_ANNOT_INK:
                    if hasattr(annot, 'stroke') and annot.stroke:
                        stroke_points = annot.stroke
                        if len(stroke_points) > 0:
                            canvas_points = []
                            for stroke in stroke_points:
                                # Limit stroke points to prevent lag
                                if len(stroke) > 100:
                                    stroke = stroke[::max(1, len(stroke)//100)]  # Sample every nth point
                                for point in stroke:
                                    px, py = self.pdf_to_canvas(point.x, point.y)
                                    canvas_points.extend([px, py])
                            if canvas_points:
                                color = '#ff0000'
                                width = 2
                                if hasattr(annot, 'colors') and annot.colors:
                                    stroke_color = annot.colors.get('stroke')
                                    if stroke_color:
                                        color = self._rgb01_to_hex(stroke_color)
                                if hasattr(annot, 'border') and annot.border:
                                    width = max(1, int(annot.border.get('width', 2)))
                                
                                self.canvas.create_line(canvas_points, fill=color, width=width, tags='annotation')
                
                elif annot.type[0] == fitz.PDF_ANNOT_FREETEXT:
                    # Draw text annotations
                    content = annot.content if annot.content else "Text"
                    self.canvas.create_text(
                        (x0 + x1) / 2, (y0 + y1) / 2,
                        text=content, fill='#0000ff', font=('Arial', 10),
                        tags='annotation'
                    )
                
                elif annot.type[0] == fitz.PDF_ANNOT_HIGHLIGHT:
                    # Draw highlight annotations
                    self.canvas.create_rectangle(
                        x0, y0, x1, y1,
                        fill='#ffff00', stipple='gray25', outline='',
                        tags='annotation'
                    )
                
                # Draw annotation border for visibility
                self.canvas.create_rectangle(
                    x0, y0, x1, y1,
                    outline='#cccccc', width=1, dash=(2, 2),
                    tags='annotation'
                )
        except Exception:
            pass

    def set_annotation_mode(self, mode):
        """Set the current annotation mode and configure UI accordingly."""
        prev_mode = self.annot_mode.get() if hasattr(self, 'annot_mode') else None
        self.annot_mode.set(mode)
        
        # Update button states
        for m, btn in self.annot_btns.items():
            if m == mode:
                btn.config(bg=BUTTON_ACTIVE, fg=FG_COLOR)
            else:
                btn.config(bg=TOOLBAR_COLOR, fg=FG_COLOR)
        
        # Update status message
        status_messages = {
            'none': 'Ready',
            'draw': 'Click and drag to draw freehand',
            'eraser': 'Click on an annotation to remove it'
        }
        self.status.config(text=status_messages.get(mode, f'Annotation mode: {mode.capitalize()}'))
        
        
        # Show/hide annotation options
        if mode == 'draw':
            self.annot_options.pack(side='right', padx=8)
        else:
            self.annot_options.pack_forget()
        
        # Clear existing canvas bindings (optimized)
        self.canvas.unbind('<Button-1>')
        self.canvas.unbind('<B1-Motion>')
        self.canvas.unbind('<ButtonRelease-1>')
        self.canvas.unbind('<Double-Button-1>')
        self.canvas.unbind('<Motion>')
        self.canvas.config(cursor='arrow')
        
        # Clear eraser hover highlight
        self.canvas.delete('eraser_hover')
        if hasattr(self, '_eraser_hover_rect'):
            del self._eraser_hover_rect
        
        # Set mode-specific bindings and cursor (optimized - no recursion)
        if mode == 'draw':
            # Check if already in draw mode for toggle behavior
            if prev_mode == 'draw':
                # Switch to none mode without recursion
                mode = 'none'
                self.annot_mode.set(mode)
                # Update button states for none mode
                for m, btn in self.annot_btns.items():
                    if m == mode:
                        btn.config(bg=BUTTON_ACTIVE, fg=FG_COLOR)
                    else:
                        btn.config(bg=TOOLBAR_COLOR, fg=FG_COLOR)
                self.status.config(text='Ready')
                self.annot_options.pack_forget()
            else:
                # Enter draw mode
                self.canvas.bind('<Button-1>', self._start_draw)
                self.canvas.bind('<B1-Motion>', self._draw_draw)
                self.canvas.bind('<ButtonRelease-1>', self._end_draw)
                self.canvas.config(cursor='pencil')
        elif mode == 'eraser':
            # Check if already in eraser mode for toggle behavior
            if prev_mode == 'eraser':
                # Switch to none mode without recursion
                mode = 'none'
                self.annot_mode.set(mode)
                # Update button states for none mode
                for m, btn in self.annot_btns.items():
                    if m == mode:
                        btn.config(bg=BUTTON_ACTIVE, fg=FG_COLOR)
                    else:
                        btn.config(bg=TOOLBAR_COLOR, fg=FG_COLOR)
                self.status.config(text='Ready')
                self.annot_options.pack_forget()
            else:
                # Enter eraser mode
                self.canvas.bind('<Button-1>', self._erase_annot)
                self.canvas.bind('<Motion>', self._highlight_eraser_hover)
                self.canvas.config(cursor='X_cursor')
        
        # Ensure canvas has focus for annotation tools (only if not page entry focused)
        if not self._page_entry_has_focus:
            self.canvas.focus_set()
        

    # --- Coordinate conversion helpers ---
    def canvas_to_pdf(self, x, y):
        """Convert canvas coordinates to PDF page coordinates."""
        pdf_image_items = self.canvas.find_withtag('pdf_image')
        if pdf_image_items:
            pdf_coords = self.canvas.coords(pdf_image_items[0])
            pdf_center_x, pdf_center_y = pdf_coords[0], pdf_coords[1]
            
            pdf_bbox = self.canvas.bbox(pdf_image_items[0])
            if pdf_bbox:
                pdf_width = pdf_bbox[2] - pdf_bbox[0]
                pdf_height = pdf_bbox[3] - pdf_bbox[1]
                
                pdf_top_left_x = pdf_center_x - (pdf_width / 2)
                pdf_top_left_y = pdf_center_y - (pdf_height / 2)
                
                px = (x - pdf_top_left_x) / self._pdf_scale
                py = (y - pdf_top_left_y) / self._pdf_scale
            else:
                px = x / self._pdf_scale
                py = y / self._pdf_scale
        else:
            px = x / self._pdf_scale
            py = y / self._pdf_scale
        return px, py
    
    def pdf_to_canvas(self, x, y):
        """Convert PDF page coordinates to canvas coordinates."""
        pdf_image_items = self.canvas.find_withtag('pdf_image')
        if pdf_image_items:
            pdf_coords = self.canvas.coords(pdf_image_items[0])
            pdf_center_x, pdf_center_y = pdf_coords[0], pdf_coords[1]
            
            pdf_bbox = self.canvas.bbox(pdf_image_items[0])
            if pdf_bbox:
                pdf_width = pdf_bbox[2] - pdf_bbox[0]
                pdf_height = pdf_bbox[3] - pdf_bbox[1]
                
                pdf_top_left_x = pdf_center_x - (pdf_width / 2)
                pdf_top_left_y = pdf_center_y - (pdf_height / 2)
                
                cx = x * self._pdf_scale + pdf_top_left_x
                cy = y * self._pdf_scale + pdf_top_left_y
            else:
                cx = x * self._pdf_scale
                cy = y * self._pdf_scale
        else:
            cx = x * self._pdf_scale
            cy = y * self._pdf_scale
        return cx, cy

    # --- Draw Tool ---
    def _start_draw(self, event):
        cx, cy = event.x, event.y
        self._draw_points = [(cx, cy)]
        self._draw_line = self.canvas.create_line(cx, cy, cx, cy, fill=self.color_var.get(), width=self.width_var.get())
        self._draw_history = []
        # Initialize drawing state
        self._last_draw_update = 0
    def _draw_draw(self, event):
        if hasattr(self, '_draw_line'):
            cx, cy = event.x, event.y
            self._draw_points.append((cx, cy))
            
            # Optimized coordinate update - only update every 5 points for performance
            if len(self._draw_points) % 5 == 0:
                coords = []
                for point in self._draw_points:
                    coords.extend(point)
                self.canvas.coords(self._draw_line, *coords)
    def _end_draw(self, event):
        if hasattr(self, '_draw_line'):
            # Final coordinate update before ending
            if hasattr(self, '_draw_points') and len(self._draw_points) > 1:
                coords = []
                for point in self._draw_points:
                    coords.extend(point)
                self.canvas.coords(self._draw_line, *coords)
            
            # Only create annotation if we have enough points (minimum 3 points)
            if hasattr(self, '_draw_points') and len(self._draw_points) >= 3:
                page = self.pdf_doc.load_page(self.current_page)
                
                pdf_points = []
                for cx, cy in self._draw_points:
                    px, py = self.canvas_to_pdf(cx, cy)
                    pdf_points.append((px, py))
                
                annot = page.add_ink_annot([pdf_points])
                rgb = self._hex_to_rgb01(self.color_var.get())
                annot.set_colors(stroke=rgb)
                annot.set_border(width=self.width_var.get())
                annot.update()
                
                # Undo functionality removed as requested
                self._save_pdf_safe(show_message=False)
                
                # Optimized: Only re-render annotations, don't reload entire page
                self._render_annotations()
            
            # Cleanup
            if hasattr(self, '_draw_line'):
                self._draw_history.append(self._draw_line)
                del self._draw_line
            if hasattr(self, '_draw_points'):
                del self._draw_points


    def _highlight_eraser_hover(self, event):
        # Highly optimized eraser hover - minimal processing
        if not hasattr(self, '_last_hover_pos'):
            self._last_hover_pos = (0, 0)
        
        # Only process if mouse moved more than 10 pixels (increased threshold for performance)
        if abs(event.x - self._last_hover_pos[0]) < 10 and abs(event.y - self._last_hover_pos[1]) < 10:
            return
        
        self._last_hover_pos = (event.x, event.y)
        
        # Remove previous highlight first
        if hasattr(self, '_eraser_hover_rect'):
            self.canvas.delete(self._eraser_hover_rect)
            del self._eraser_hover_rect
        
        # Only check annotations if PDF is loaded and we're in eraser mode
        if not self.pdf_doc or (hasattr(self, 'annot_mode') and self.annot_mode.get() != 'eraser'):
            return
            
        try:
            # Use cached page if available to avoid repeated loading
            if not hasattr(self, '_cached_page') or self._cached_page_num != self.current_page:
                self._cached_page = self.pdf_doc.load_page(self.current_page)
                self._cached_page_num = self.current_page
            
            page = self._cached_page
            
            try:
                x, y = self.canvas.canvasx(event.x), self.canvas.canvasy(event.y)
            except:
                x, y = event.x, event.y
            
            # Convert to PDF coordinates for annotation checking
            px, py = self.canvas_to_pdf(x, y)
            
            # Limit annotation checking to prevent lag
            annotations = list(page.annots())
            if len(annotations) > 20:  # Only check first 20 annotations
                annotations = annotations[:20]
            
            found = None
            for annot in annotations:
                rect = annot.rect
                if rect.contains(fitz.Point(px, py)):
                    found = rect
                    break
            
            if found:
                # Convert back to canvas coordinates for highlighting
                x0, y0 = self.pdf_to_canvas(found.x0, found.y0)
                x1, y1 = self.pdf_to_canvas(found.x1, found.y1)
                
                # Draw highlight rectangle
                self._eraser_hover_rect = self.canvas.create_rectangle(
                    x0, y0, x1, y1,
                    outline='red', width=2, dash=(3,2), tags='eraser_hover')
        except:
            pass  # Ignore errors to prevent lag

    def _hex_to_rgb01(self, hex_color):
        hex_color = hex_color.lstrip('#')
        lv = len(hex_color)
        if lv == 6:
            return tuple(int(hex_color[i:i+2], 16)/255.0 for i in (0, 2, 4))
        elif lv == 3:
            return tuple(int(hex_color[i]*2, 16)/255.0 for i in range(3))
        else:
            raise ValueError("Invalid hex color")
    
    def _rgb01_to_hex(self, rgb_tuple):
        """Convert RGB tuple (0-1 range) to hex color."""
        if not rgb_tuple or len(rgb_tuple) != 3:
            return '#ff0000'  # default red
        r, g, b = rgb_tuple
        return f"#{int(r*255):02x}{int(g*255):02x}{int(b*255):02x}"

    def _erase_annot(self, event):
        # Find and delete annotation under cursor (improved hit-test for ink)
        page = self.pdf_doc.load_page(self.current_page)
        try:
            x = self.canvas.canvasx(self.winfo_pointerx() - self.canvas.winfo_rootx())
            y = self.canvas.canvasy(self.winfo_pointery() - self.canvas.winfo_rooty())
        except:
            x = y = 0
        px, py = self.canvas_to_pdf(x, y)
        found = False
        for annot in page.annots():
            rect = annot.rect
            if rect.contains(fitz.Point(px, py)):
                found = True
            elif annot.type[0] == fitz.PDF_ANNOT_INK and self._is_point_near_ink(annot, px, py, tolerance=14):
                found = True
            if found:
                # Undo functionality removed as requested
                page.delete_annot(annot)
                break
        if hasattr(self, '_eraser_hover_rect'):
            self.canvas.delete(self._eraser_hover_rect)
            del self._eraser_hover_rect
        if found:
            # Annotation erased
            self._save_pdf_safe(show_message=False)
            # Optimized: Only re-render annotations, don't reload entire page
            self._render_annotations()
        else:
            # Create a centered message box for eraser
            msg_box = tk.Toplevel(self)
            msg_box.title('Eraser')
            msg_box.configure(bg=BG_COLOR)
            msg_box.geometry('300x100')
            msg_box.resizable(False, False)
            
            # Center the message box
            msg_box.transient(self)
            msg_box.grab_set()
            
            # Center on screen
            # Removed update_idletasks for performance
            x = (msg_box.winfo_screenwidth() // 2) - (msg_box.winfo_width() // 2)
            y = (msg_box.winfo_screenheight() // 2) - (msg_box.winfo_height() // 2)
            msg_box.geometry(f"+{x}+{y}")
            
            # Message label
            tk.Label(msg_box, text='No annotation found at this location.', 
                    bg=BG_COLOR, fg=FG_COLOR, font=FONT).pack(expand=True)
            
            # OK button
            tk.Button(msg_box, text='OK', command=msg_box.destroy,
                     bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT, relief='flat',
                     padx=20).pack(pady=10)
            
            # Auto close after 2 seconds
            msg_box.after(2000, msg_box.destroy)

    def _is_point_near_ink(self, annot, px, py, tolerance=8):
        # px, py are in PDF coordinates
        # Check if (px, py) is within 'tolerance' points of any segment in the ink annotation
        if annot.type[0] == fitz.PDF_ANNOT_INK:
            for path in annot.vertices:
                for i in range(len(path) - 1):
                    x0, y0 = path[i]
                    x1, y1 = path[i+1]
                    if self._point_to_segment_dist(px, py, x0, y0, x1, y1) <= tolerance:
                        return True
        return False

    def _point_to_segment_dist(self, px, py, x0, y0, x1, y1):
        # Return the distance from (px, py) to the segment (x0, y0)-(x1, y1)
        from math import hypot
        dx, dy = x1 - x0, y1 - y0
        if dx == dy == 0:
            return hypot(px - x0, py - y0)
        t = max(0, min(1, ((px - x0) * dx + (py - y0) * dy) / (dx*dx + dy*dy)))
        proj_x = x0 + t * dx
        proj_y = y0 + t * dy
        return hypot(px - proj_x, py - proj_y)



    def _disable_form_fields(self):
        """Disable form fields and remove widgets."""
        if hasattr(self, '_form_entries'):
            for entry in self._form_entries:
                if isinstance(entry, tuple):
                    self.canvas.delete(entry[1])
                    entry[0].destroy()
                else:
                    entry.destroy()
            self._form_entries.clear()

    def show_license(self):
            license_win = tk.Toplevel(self)
            license_win.title("License")
            license_win.configure(bg=BG_COLOR)
            license_win.resizable(False, False)
            try:
                license_win.iconbitmap(ICON_PATH)
            except Exception:
                pass

            license_text = (
                "This software is licensed under the MIT License.\n\n"
                "Permission is hereby granted, free of charge, to any person obtaining a copy "
                "of this software and associated documentation files (the \"Software\"), to deal "
                "in the Software without restriction, including without limitation the rights "
                "to use, copy, modify, merge, publish, distribute, sublicense, and/or sell "
                "copies of the Software, and to permit persons to whom the Software is "
                "furnished to do so, subject to the following conditions:\n\n"
                "The above copyright notice and this permission notice shall be included in all "
                "copies or substantial portions of the Software.\n\n"
                "THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR "
                "IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, "
                "FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE "
                "AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER "
                "LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, "
                "OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE "
                "SOFTWARE."
            )

            text_label = tk.Label(
                license_win,
                text=license_text,
                bg=BG_COLOR,
                fg="white",
                justify="left",
                anchor="nw",
                wraplength=400
            )
            text_label.pack(padx=15, pady=(15, 5))

            # Hyperlink label
            link_text = "For license and project info, click here: https://pdf-reader.ansnew.com"
            link_label = tk.Label(
                license_win,
                text=link_text,
                fg="cyan",
                bg=BG_COLOR,
                cursor="hand2",
                wraplength=400,
                justify="left"
            )
            link_label.pack(padx=15, pady=(0, 15))

            def open_link(event):
                webbrowser.open_new("https://pdf-reader.ansnew.com")

            link_label.bind("<Button-1>", open_link)


    def show_about(self):
        about_win = tk.Toplevel(self)
        about_win.title("About")
        about_win.configure(bg=BG_COLOR)
        about_win.resizable(False, False)
        try:
            about_win.iconbitmap(ICON_PATH)
        except Exception:
            pass
        # Load photo
        try:
            img_path = os.path.join(ASSET_DIR)
            img = Image.open(img_path)
            img = img.resize((120, 120), Image.LANCZOS)
            photo = ImageTk.PhotoImage(img)
        except Exception as e:
            photo = None

        frame = tk.Frame(about_win, bg=BG_COLOR)
        frame.pack(padx=24, pady=24)

        if photo:
            img_label = tk.Label(frame, image=photo, bg=BG_COLOR)
            img_label.image = photo  # Keep reference
            img_label.grid(row=0, column=0, rowspan=6, padx=(0, 20), sticky='n')

        # Your info
        software_name = "Advanced PDF Reader"
        name = "Yamin Hossain"
        title = "Software Engineer, Python Developer"
        email = "needyamin@gmail.com"
        github = "github.com/needyamin"
        # linkedin = "linkedin.com/in/yaminhossain"  # Removed
        about_text = (
            "Advanced PDF Reader is a modern, professional PDF reader and annotator for Windows. "
            "It was created to provide a fast, beautiful, and feature-rich alternative to basic PDF viewers, "
            "with a focus on annotation, form filling, and a Photoshop-inspired dark UI."
        )
        why_text = (
            "I created this software to help students, professionals, and anyone who needs to read, annotate, "
            "and fill forms in PDFs with ease. My goal was to combine a modern user experience with powerful features, "
            "all in a free and open source package."
        )

        tk.Label(frame, text=software_name, font=('Segoe UI', 15, 'bold'), bg=BG_COLOR, fg=ACCENT_COLOR).grid(row=0, column=1, sticky='w', pady=(0, 2))
        tk.Label(frame, text=f"by {name}", font=FONT, bg=BG_COLOR, fg=FG_COLOR).grid(row=1, column=1, sticky='w')
        tk.Label(frame, text=title, font=FONT, bg=BG_COLOR, fg=FG_COLOR).grid(row=2, column=1, sticky='w')
        tk.Label(frame, text=f"Email: {email}", font=FONT, bg=BG_COLOR, fg=FG_COLOR).grid(row=3, column=1, sticky='w')
        tk.Label(frame, text=f"GitHub: {github}", font=FONT, bg=BG_COLOR, fg=FG_COLOR, cursor='hand2').grid(row=4, column=1, sticky='w')
        # Removed LinkedIn row

        # About this software
        about_frame = tk.Frame(about_win, bg=BG_COLOR)
        about_frame.pack(padx=24, pady=(0, 12), fill='x')
        tk.Label(about_frame, text="About this software:", font=('Segoe UI', 12, 'bold'), bg=BG_COLOR, fg=ACCENT_COLOR).pack(anchor='w')
        tk.Label(about_frame, text=about_text, font=FONT, bg=BG_COLOR, fg=FG_COLOR, wraplength=400, justify='left').pack(anchor='w', pady=(0, 8))
        tk.Label(about_frame, text="Why I created this:", font=('Segoe UI', 12, 'bold'), bg=BG_COLOR, fg=ACCENT_COLOR).pack(anchor='w')
        tk.Label(about_frame, text=why_text, font=FONT, bg=BG_COLOR, fg=FG_COLOR, wraplength=400, justify='left').pack(anchor='w')

        # Optionally, add clickable links
        def open_url(url):
            import webbrowser
            webbrowser.open(url)

        frame.grid_slaves(row=4, column=1)[0].bind("<Button-1>", lambda e: open_url("https://github.com/needyamin"))
        # Removed LinkedIn link

        tk.Button(about_win, text="Close", command=about_win.destroy, bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT, relief='flat').pack(pady=(8, 0))

        about_win.grab_set()
        about_win.transient(self)
        about_win.focus_set()

    def show_shortcuts(self):
        shortcuts = [
            ('Open PDF', 'Ctrl+O'),
            ('Save PDF', 'Ctrl+S'),
            ('Save As', 'Ctrl+Shift+S'),
            ('Export as Image', 'File > Export as Image'),
            ('Exit', 'Ctrl+Q, Esc'),
            ('Next Page', 'Right, PageDown, Space'),
            ('Previous Page', 'Left, PageUp, Shift+Space'),
            ('First Page', 'Home'),
            ('Last Page', 'End'),
            ('Scroll Up', 'Up Arrow'),
            ('Scroll Down', 'Down Arrow'),
            ('Scroll Left', 'Ctrl + Left Arrow'),
            ('Scroll Right', 'Ctrl + Right Arrow'),
            ('Fast Scroll', 'Shift + Arrow Keys'),
            ('Zoom In', 'Ctrl++, +, ='),
            ('Zoom Out', 'Ctrl+-, -'),
            ('Rotate', 'R'),
            ('Fit to Window', 'F, Ctrl+F'),
            ('Go to Page', 'G, Enter'),
            ('Toggle Sidebar', 'S, Ctrl+B, Esc'),
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
        try:
            top.iconbitmap(ICON_PATH)
        except Exception:
            pass
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
                try:
                    # Only generate event if the window is still running
                    if self.winfo_exists():
                        self.event_generate('<<ReallyExit>>', when='tail')
                except Exception as e:
                    # Optionally log or ignore
                    pass
            try:
                icon_img = PILImage.open(ICON_PATH)
                # Resize to proper tray icon size
                icon_img = icon_img.resize((64, 64), PILImage.Resampling.LANCZOS)
            except Exception as e:
                pass  # Tray icon failed
                # Create a proper default icon
                icon_img = PILImage.new('RGBA', (64, 64), color=(0, 100, 200, 255))
                # Add a simple "P" text
                from PIL import ImageDraw, ImageFont
                draw = ImageDraw.Draw(icon_img)
                try:
                    font = ImageFont.truetype("arial.ttf", 40)
                except:
                    font = ImageFont.load_default()
                draw.text((20, 12), "P", fill=(255, 255, 255, 255), font=font)
            menu = pystray.Menu(
                pystray.MenuItem('Show', show_window),
                pystray.MenuItem('Exit', quit_app)
            )
            self.tray_icon = pystray.Icon('PDFReader', icon_img, 'PDF Reader', menu)
            # Start the tray icon in a separate thread
            tray_thread = threading.Thread(target=self.tray_icon.run, daemon=True)
            tray_thread.start()
            # System tray started
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
        try:
            x = self.canvas.canvasx(self.winfo_pointerx() - self.canvas.winfo_rootx())
            y = self.canvas.canvasy(self.winfo_pointery() - self.canvas.winfo_rooty())
        except:
            x = y = 0
        found = False
        for annot in page.annots():
            rect = annot.rect
            if rect.contains(fitz.Point(*self.canvas_to_pdf(x, y))):
                # Undo functionality removed as requested
                page.delete_annot(annot)
                found = True
                break
        if found:
            try:
                self.pdf_doc.saveIncr()
            except Exception as e:
                logger.warning(f"Incremental save failed, using full save: {e}")
                # Save to temporary file and replace original
                import tempfile
                with tempfile.NamedTemporaryFile(delete=False, suffix='.pdf') as tmp_file:
                    tmp_path = tmp_file.name
                self.pdf_doc.save(tmp_path, incremental=False, encryption=False)
                import shutil
                shutil.move(tmp_path, self.pdf_doc.name)
                # Reload the document to reflect changes
                self.pdf_doc = fitz.open(self.pdf_doc.name)
            self.show_page()
            self.status.config(text='Annotation deleted (Delete key).')
        else:
            self.status.config(text='No annotation detected under mouse.')

    # Copy/paste annotation methods removed as requested

    def _save_last_session(self, file_path, page_num):
        """Save current session to persistent storage"""
        try:
            # Ensure directory exists
            os.makedirs(os.path.dirname(SESSION_FILE), exist_ok=True)
            
            session_data = {
                'file': file_path,
                'page': page_num,
                'zoom': self.zoom,
                'rotation': self.rotation,
                'fit_to_window': self.fit_to_window.get(),
                'sidebar_visible': self.sidebar_visible,
                'timestamp': time.time()
            }
            
            with open(SESSION_FILE, 'w') as f:
                json.dump(session_data, f, indent=2)
        except Exception as e:
            pass  # Session save failed

    def _load_last_session(self):
        """Load last session from persistent storage"""
        if self._session_loaded:
            return
        self._session_loaded = True
        
        try:
            if not os.path.exists(SESSION_FILE):
                return
                
            with open(SESSION_FILE, 'r') as f:
                data = json.load(f)
                
            file_path = data.get('file')
            page_num = data.get('page', 0)
            
            # Restore other settings
            self.zoom = data.get('zoom', 1.0)
            self.rotation = data.get('rotation', 0)
            self.fit_to_window.set(data.get('fit_to_window', True))
            self.sidebar_visible = data.get('sidebar_visible', True)
            
            if file_path and os.path.exists(file_path):
                # Restoring last session
                self.open_pdf(file_path)
                self.after(500, lambda: self.go_to_page(page_num))
        except Exception as e:
            pass  # Session load failed

    def exit_app(self):
        # If you want to always fully exit:
        self.destroy()
        import sys
        sys.exit(0)

    def get_github_token(self):
        token = os.environ.get('GITHUB_TOKEN')
        if token:
            return token
        try:
            with open(TOKEN_FILE, 'r') as f:
                data = json.load(f)
                return data.get('token')
        except Exception:
            return None

    def check_for_update(self):
        token = self.get_github_token()
        headers = {'Accept': 'application/vnd.github.v3+json'}
        if token:
            headers['Authorization'] = f'token {token}'
        url = f'https://api.github.com/repos/{GITHUB_REPO}/releases/latest'
        try:
            resp = requests.get(url, headers=headers, timeout=10)
            if resp.status_code == 200:
                data = resp.json()
                latest_version = data['tag_name'].lstrip('v')
                if latest_version > __version__:
                    asset = next((a for a in data['assets'] if a['name'].endswith('.exe')), None)
                    if asset:
                        download_url = asset['browser_download_url']
                        return latest_version, download_url
            return None, None
        except Exception as e:
            pass  # Update check failed
            return None, None

    def download_and_replace(self, download_url):
        exe_path = sys.argv[0]
        tmp_path = exe_path + ".new"
        try:
            with requests.get(download_url, stream=True) as r:
                with open(tmp_path, 'wb') as f:
                    for chunk in r.iter_content(chunk_size=8192):
                        if chunk:
                            f.write(chunk)
            # On Windows, can't overwrite running exe; so schedule replace on next start
            messagebox.showinfo("Update", f"Update downloaded to {tmp_path}. Please close the app and replace the old .exe with the new one.")
        except Exception as e:
            messagebox.showerror("Update Failed", f"Failed to download update: {e}")

    def auto_update(self, manual=False):
        latest_version, download_url = self.check_for_update()
        if latest_version and download_url:
            if messagebox.askyesno("Update Available", f"A new version ({latest_version}) is available. Download and update now?"):
                self.download_and_replace(download_url)
        else:
            if manual and threading.current_thread() is threading.main_thread():
                messagebox.showinfo("No Update", "You are using the latest version.")

    def _on_canvas_mousewheel(self, event):
        # Main PDF scroll: much more responsive (20 units)
        if event.state & 0x1:  # Shift is held
            if hasattr(event, 'delta'):
                if event.delta > 0:
                    self.canvas.xview_scroll(-20, 'units')
                elif event.delta < 0:
                    self.canvas.xview_scroll(20, 'units')
            else:
                if event.num == 4:
                    self.canvas.xview_scroll(-20, 'units')
                elif event.num == 5:
                    self.canvas.xview_scroll(20, 'units')
        else:
            if hasattr(event, 'delta'):
                if event.delta > 0:
                    self.canvas.yview_scroll(-20, 'units')
                elif event.delta < 0:
                    self.canvas.yview_scroll(20, 'units')
            else:
                if event.num == 4:
                    self.canvas.yview_scroll(-20, 'units')
                elif event.num == 5:
                    self.canvas.yview_scroll(20, 'units')
        
        # Update scroll position tracking
        try:
            self._last_scroll_x = self.canvas.canvasx(0)
            self._last_scroll_y = self.canvas.canvasy(0)
        except:
            self._last_scroll_x = self._last_scroll_y = 0

    def _on_canvas_key_scroll(self, direction, amount=20):
        """Handle keyboard scrolling of the main PDF canvas"""
        if direction == 'up':
            self.canvas.yview_scroll(-amount, 'units')
        elif direction == 'down':
            self.canvas.yview_scroll(amount, 'units')
        elif direction == 'left':
            self.canvas.xview_scroll(-amount, 'units')
        elif direction == 'right':
            self.canvas.xview_scroll(amount, 'units')
        
        # Update scroll position tracking
        try:
            self._last_scroll_x = self.canvas.canvasx(0)
            self._last_scroll_y = self.canvas.canvasy(0)
        except:
            self._last_scroll_x = self._last_scroll_y = 0

    def _propagate_mousewheel_to_canvas(self, widget, canvas):
        # Ensure mouse wheel events on widget and all children are handled by canvas
        def add_bindings(w):
            tags = list(w.bindtags())
            if str(canvas) not in tags:
                tags.insert(1, str(canvas))
                w.bindtags(tuple(tags))
            # Recursively add to children
            for child in getattr(w, 'winfo_children', lambda:[])():
                add_bindings(child)
        add_bindings(widget)

    def toggle_fullscreen(self):
        self._fullscreen = not self._fullscreen
        self.attributes('-fullscreen', self._fullscreen)
        if self._fullscreen:
            self.status.config(text='Full Screen Mode (Press Esc to exit)')
        else:
            self.status.config(text='Exited Full Screen')

    def exit_fullscreen(self):
        if self._fullscreen:
            self._fullscreen = False
            self.attributes('-fullscreen', False)
            self.status.config(text='Exited Full Screen')

    def _save_pdf_safe(self, show_message=True):
        """Safe PDF save method that handles all edge cases."""
        if not self.pdf_doc or not self._current_pdf_path:
            if show_message:
                messagebox.showwarning('No PDF', 'No PDF loaded to save.')
            return False
        
        try:
            # Try incremental save first
            self.pdf_doc.saveIncr()
                # PDF saved incrementally
            if show_message:
                self.status.config(text=f'Saved: {os.path.basename(self._current_pdf_path)}')
                messagebox.showinfo('Saved', f'PDF saved successfully:\n{os.path.basename(self._current_pdf_path)}')
            return True
        except Exception as e:
            # Incremental save failed, trying full save
            try:
                # Use temporary file approach for full save
                import tempfile
                import shutil
                
                # Create temporary file
                with tempfile.NamedTemporaryFile(delete=False, suffix='.pdf') as tmp_file:
                    tmp_path = tmp_file.name
                
                # Save to temporary file
                self.pdf_doc.save(tmp_path, incremental=False, encryption=False)
                
                # Replace original file
                shutil.move(tmp_path, self._current_pdf_path)
                
                # Reload the document to reflect changes
                self.pdf_doc = fitz.open(self._current_pdf_path)
                
                # PDF saved with full save
                if show_message:
                    self.status.config(text=f'Saved: {os.path.basename(self._current_pdf_path)}')
                    messagebox.showinfo('Saved', f'PDF saved successfully:\n{os.path.basename(self._current_pdf_path)}')
                return True
            except Exception as e2:
                # Full save also failed
                if show_message:
                    messagebox.showerror('Save Error', f'Could not save PDF:\n{str(e2)}')
                return False

    def save_pdf(self):
        """Save PDF with user feedback."""
        self._save_pdf_safe(show_message=True)

    def save_pdf_as(self):
        """Save PDF to a new location (Save As)."""
        if not self.pdf_doc:
            messagebox.showwarning('No PDF', 'No PDF loaded to save.')
            return
        
        # Ask user for save location
        from tkinter import filedialog
        file_path = filedialog.asksaveasfilename(
            title='Save PDF As',
            defaultextension='.pdf',
            filetypes=[
                ('PDF files', '*.pdf'),
                ('All files', '*.*')
            ],
            initialdir=os.path.dirname(self._current_pdf_path) if self._current_pdf_path else None,
            initialfile=os.path.basename(self._current_pdf_path).replace('.pdf', '_copy.pdf') if self._current_pdf_path else 'document.pdf'
        )
        
        if not file_path:
            return  # User cancelled
        
        try:
            # Save to new location
            self.pdf_doc.save(file_path, incremental=False, encryption=False)
            # PDF saved as
            self.status.config(text=f'Saved as: {os.path.basename(file_path)}')
            messagebox.showinfo('Saved', f'PDF saved successfully as:\n{os.path.basename(file_path)}')
        except Exception as e:
            # Save As failed
            messagebox.showerror('Save Error', f'Could not save PDF:\n{str(e)}')

    def _check_license(self):
        # Check license file for start date and license key
        now = datetime.datetime.now()
        try:
            with open(LICENSE_FILE, 'r') as f:
                data = json.load(f)
        except Exception:
            data = {}
        start_str = data.get('start_date')
        license_key = data.get('license_key')
        if not start_str:
            # First run: store start date
            data['start_date'] = now.strftime('%Y-%m-%d')
            with open(LICENSE_FILE, 'w') as f:
                json.dump(data, f)
            return
        start_date = datetime.datetime.strptime(start_str, '%Y-%m-%d')
        days_used = (now - start_date).days
        if days_used < 7:
            return
        # After 7 days, require license
        if not license_key or not self._validate_license(license_key):
            self._prompt_license()

    def _validate_license(self, key):
        # Simple example: valid key is 'NEEDYAMIN-2025'
        return key == 'NEEDYAMIN-2025'

    def _prompt_license(self):
        import tkinter.messagebox as mb
        # Custom modal dialog for license entry with app icon
        for _ in range(3):
            key = self._show_license_dialog()
            if key and self._validate_license(key):
                # Save license
                try:
                    with open(LICENSE_FILE, 'r') as f:
                        data = json.load(f)
                except Exception:
                    data = {}
                data['license_key'] = key
                with open(LICENSE_FILE, 'w') as f:
                    json.dump(data, f)
                mb.showinfo('License Activated', 'Thank you! License activated.', parent=self)
                return
            else:
                mb.showerror('Invalid License', 'The license key is invalid.', parent=self)
        mb.showerror('License Required', 'No valid license entered. The app will now exit.', parent=self)
        import sys
        self.destroy()
        sys.exit(1)

    def _show_license_dialog(self):
        # Create a modal dialog with app icon for license entry
        dialog = tk.Toplevel(self)
        dialog.title('License Required')
        dialog.configure(bg=BG_COLOR)
        dialog.resizable(False, False)
        try:
            dialog.iconbitmap(ICON_PATH)
        except Exception:
            pass
        dialog.transient(self)
        dialog.grab_set()
        dialog.lift()
        dialog.attributes('-topmost', True)
        # Center dialog
        # Removed update_idletasks for performance
        w, h = 340, 160
        x = self.winfo_rootx() + (self.winfo_width() - w) // 2
        y = self.winfo_rooty() + (self.winfo_height() - h) // 2
        dialog.geometry(f'{w}x{h}+{x}+{y}')
        # Widgets
        label = tk.Label(dialog, text='Your trial has expired.\nPlease enter your license key:', bg=BG_COLOR, fg=FG_COLOR, font=FONT, justify='center')
        label.pack(pady=(18, 8), padx=16)
        entry_var = tk.StringVar()
        entry = tk.Entry(dialog, textvariable=entry_var, font=FONT, width=28, justify='center')
        entry.pack(pady=(0, 12))
        entry.focus_set()
        result = {'key': None}
        def submit():
            result['key'] = entry_var.get().strip()
            dialog.destroy()
        def cancel():
            result['key'] = None
            dialog.destroy()
        btn_frame = tk.Frame(dialog, bg=BG_COLOR)
        btn_frame.pack(pady=(0, 10))
        ok_btn = tk.Button(btn_frame, text='OK', command=submit, font=FONT, bg=TOOLBAR_COLOR, fg=FG_COLOR, relief='flat', width=8)
        ok_btn.pack(side='left', padx=8)
        cancel_btn = tk.Button(btn_frame, text='Cancel', command=cancel, font=FONT, bg=TOOLBAR_COLOR, fg=FG_COLOR, relief='flat', width=8)
        cancel_btn.pack(side='left', padx=8)
        dialog.bind('<Return>', lambda e: submit())
        dialog.bind('<Escape>', lambda e: cancel())
        self.wait_window(dialog)
        return result['key']

if __name__ == '__main__':
    # Check for command-line arguments (PDF file to open)
    pdf_file_to_open = None
    if len(sys.argv) > 1:
        pdf_file_to_open = sys.argv[1]
        # Validate that it's a PDF file
        if not pdf_file_to_open.lower().endswith('.pdf'):
            pdf_file_to_open = None
    
    # Splash is the root window
    splash = tk.Tk()
    splash.overrideredirect(True)
    splash.configure(bg='#23272e')
    try:
        splash_img = Image.open(LOADING_IMG_PATH)
        img_w, img_h = splash_img.size
        splash_photo = ImageTk.PhotoImage(splash_img)
    except Exception:
        splash_photo = None
        img_w, img_h = 200, 200
    screen_w = splash.winfo_screenwidth()
    screen_h = splash.winfo_screenheight()
    x = (screen_w - img_w) // 2
    y = (screen_h - img_h) // 2
    splash.geometry(f'{img_w}x{img_h}+{x}+{y}')
    if splash_photo:
        label = tk.Label(splash, image=splash_photo, bg='#23272e', borderwidth=0, highlightthickness=0)
        label.pack(expand=True, fill='both')
    else:
        label = tk.Label(splash, text='Loading...', font=('Segoe UI', 24), bg='#23272e', fg='white')
        label.pack(expand=True, fill='both')
    splash.update()

    def show_main():
        try:
            splash.destroy()
        except:
            pass
        
        app = PDFReaderApp()
        
        # Ensure window is visible and on top BEFORE loading PDF
        app.deiconify()  # Show window if minimized
        app.lift()       # Bring to front
        app.focus_force()  # Force focus
        app.attributes('-topmost', True)  # Stay on top temporarily
        app.after(100, lambda: app.attributes('-topmost', False))  # Remove topmost after 100ms
        
        # Update the window to ensure it's visible
        app.update()
        
        # If a PDF file was passed as argument, open it instead of loading session
        if pdf_file_to_open and os.path.exists(pdf_file_to_open):
            app.after(200, lambda: app.open_pdf(pdf_file_to_open))
        
        app.mainloop()

    splash.after(1000, show_main)
    splash.mainloop()

