import os
import sys
import subprocess
import re
import tkinter as tk
from tkinter import ttk

REQUIREMENTS_TXT = 'requirements.txt'
MAIN_SCRIPT = 'pdfReader.py'
ICON = os.path.join('assets', 'icons', 'icon.ico')

# Define colors (assuming these are defined elsewhere or will be added)
BG_COLOR = "#2D3436"
ACCENT_COLOR = "#00B894"
FG_COLOR = "#FFFFFF"
TOOLBAR_COLOR = "#2D3436"
FONT = ("Segoe UI", 10)

# Define asset directories (assuming these are defined elsewhere or will be added)
ASSET_DIR = os.path.dirname(os.path.abspath(__file__))


def get_version():
    with open(MAIN_SCRIPT, 'r', encoding='utf-8') as f:
        for line in f:
            m = re.match(r'__version__\s*=\s*[\'\"]([^\'\"]+)[\'\"]', line)
            if m:
                return m.group(1)
    return "0.0.0"

def ensure_requirements_txt():
    REQUIREMENTS = [
        'PyMuPDF',
        'Pillow',
        'pywin32',
        'pystray',
        'requests',
    ]
    if not os.path.exists(REQUIREMENTS_TXT):
        with open(REQUIREMENTS_TXT, 'w') as f:
            for pkg in REQUIREMENTS:
                f.write(pkg + '\n')
        print(f'Created {REQUIREMENTS_TXT}')
    else:
        print(f'{REQUIREMENTS_TXT} already exists.')

def install_requirements():
    print('Installing requirements...')
    subprocess.check_call([sys.executable, '-m', 'pip', 'install', '-r', REQUIREMENTS_TXT])
    print('All requirements installed.')

def build_exe():
    version = get_version()
    exe_name = f'PDFReader-v{version}.exe'
    print(f'Building .exe with PyInstaller for version {version}...')
    cmd = [
        sys.executable, '-m', 'PyInstaller',
        '--onefile', '--windowed',
        f'--icon={ICON}' if os.path.exists(ICON) else '',
        '--name', exe_name.replace('.exe',''),
        MAIN_SCRIPT
    ]
    cmd = [arg for arg in cmd if arg]
    subprocess.check_call(cmd)
    print(f'Build complete! Find your .exe in dist/{exe_name}')
    # Optionally copy to release/
    release_dir = 'release'
    os.makedirs(release_dir, exist_ok=True)
    src = os.path.join('dist', exe_name)
    dst = os.path.join(release_dir, exe_name)
    if os.path.exists(src):
        import shutil
        shutil.copy2(src, dst)
        print(f'Copied to {dst}')

def show_about(self):
    about_win = tk.Toplevel(self)
    about_win.title("About")
    about_win.configure(bg=BG_COLOR)
    about_win.resizable(False, False)
    # Load photo
    try:
        from PIL import Image, ImageTk
        img_path = os.path.join(ASSET_DIR, 'images', 'YAMiN_HOSSAIN.png')
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
        img_label.grid(row=0, column=0, rowspan=4, padx=(0, 20), sticky='n')

    # Your info
    name = "Yamin Hossain"
    title = "Software Engineer, Python Developer"
    email = "needyamin@gmail.com"
    github = "github.com/needyamin"

    tk.Label(frame, text=name, font=('Segoe UI', 16, 'bold'), bg=BG_COLOR, fg=ACCENT_COLOR).grid(row=0, column=1, sticky='w')
    tk.Label(frame, text=title, font=FONT, bg=BG_COLOR, fg=FG_COLOR).grid(row=1, column=1, sticky='w')
    tk.Label(frame, text=f"Email: {email}", font=FONT, bg=BG_COLOR, fg=FG_COLOR).grid(row=2, column=1, sticky='w')
    tk.Label(frame, text=f"GitHub: {github}", font=FONT, bg=BG_COLOR, fg=FG_COLOR, cursor='hand2').grid(row=3, column=1, sticky='w')


    # Optionally, add clickable links
    def open_url(url):
        import webbrowser
        webbrowser.open(url)

    frame.grid_slaves(row=3, column=1)[0].bind("<Button-1>", lambda e: open_url("https://github.com/needyamin"))

    tk.Button(about_win, text="Close", command=about_win.destroy, bg=TOOLBAR_COLOR, fg=FG_COLOR, font=FONT, relief='flat').pack(pady=(8, 0))

    about_win.grab_set()
    about_win.transient(self)
    about_win.focus_set()

def main():
    ensure_requirements_txt()
    install_requirements()
    build_exe()

if __name__ == '__main__':
    main() 