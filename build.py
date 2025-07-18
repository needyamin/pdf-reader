import os
import sys
import subprocess
import re
import shutil

REQUIREMENTS_TXT = 'requirements.txt'
MAIN_SCRIPT = 'pdfReader.py'
ICON = os.path.join('assets', 'icons', 'icon.ico')
PUBLIC_DIR = 'public'

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

def get_add_data_arg(src, dest):
    # On Windows, use ';' as separator
    return f'{src};{dest}'

def build_exe():
    version = get_version()
    exe_name = f'PDFReader-v{version}.exe'
    print(f'Building .exe with PyInstaller for version {version}...')
    # --add-data assets;assets bundles all assets, images, icons, and json files
    cmd = [
        sys.executable, '-m', 'PyInstaller',
        '--onefile', '--windowed',
        f'--icon={ICON}' if os.path.exists(ICON) else '',
        '--name', exe_name.replace('.exe',''),
        '--distpath', os.path.join(PUBLIC_DIR, 'dist'),
        '--workpath', os.path.join(PUBLIC_DIR, 'build'),
        '--add-data', 'assets;assets',
        MAIN_SCRIPT
    ]
    cmd = [arg for arg in cmd if arg]
    subprocess.check_call(cmd)
    print(f'Build complete! Find your .exe in public/dist/{exe_name}')
    # Copy to public/
    os.makedirs(PUBLIC_DIR, exist_ok=True)
    src = os.path.join(PUBLIC_DIR, 'dist', exe_name)
    dst = os.path.join(PUBLIC_DIR, exe_name)
    if os.path.exists(src):
        shutil.copy2(src, dst)
        print(f'Copied to {dst}')
    # Ensure assets are in public/assets for Inno Setup
    public_assets = os.path.join(PUBLIC_DIR, 'assets')
    if not os.path.exists(public_assets):
        shutil.copytree('assets', public_assets)
        print(f'Copied assets/ to {public_assets}')
    return dst

def create_inno_setup_script(exe_path, version):
    iss_content = f'''
[Setup]
AppName=Advanced PDF Reader
AppVersion={version}
DefaultDirName={{{{pf}}}}\\AdvancedPDFReader
DefaultGroupName=Advanced PDF Reader
OutputDir=public
OutputBaseFilename=AdvancedPDFReader-Setup-v{version}
SetupIconFile={ICON}
Compression=lzma
SolidCompression=yes

[Files]
Source: \"{os.path.basename(exe_path)}\"; DestDir: \"{{app}}\"; Flags: ignoreversion

[Icons]
Name: \"{{group}}\\Advanced PDF Reader\"; Filename: \"{{app}}\\{os.path.basename(exe_path)}\"
Name: \"{{userdesktop}}\\Advanced PDF Reader\"; Filename: \"{{app}}\\{os.path.basename(exe_path)}\"; Tasks: desktopicon

[Tasks]
Name: \"desktopicon\"; Description: \"Create a &desktop icon\"; GroupDescription: \"Additional icons:\"
'''
    iss_path = os.path.join(PUBLIC_DIR, f'installer.iss')
    with open(iss_path, 'w', encoding='utf-8') as f:
        f.write(iss_content)
    print(f'Inno Setup script written to {iss_path}')
    return iss_path

def run_inno_setup(iss_path):
    # Common paths where ISCC.exe might be installeds
    iscc_paths = [
        r'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',  # User's confirmed path
        'ISCC',  # If in PATH
        r'C:\Program Files\Inno Setup 6\ISCC.exe',
        r'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
        r'C:\Program Files\Inno Setup 6\ISCC.exe'
    ]
    
    print('Searching for Inno Setup Compiler (ISCC.exe)...')
    iscc_found = None
    for path in iscc_paths:
        print(f'  Checking: {path}')
        try:
            if path == 'ISCC':
                # Check if ISCC is in PATH
                result = subprocess.run([path, '/?'], capture_output=True, text=True, timeout=5)
                if result.returncode in [0, 1]:  # Both 0 and 1 are valid for help
                    iscc_found = path
                    print(f'    Found ISCC in PATH')
                    break
            else:
                # Check if file exists and is executable
                if os.path.exists(path):
                    print(f'    File exists, testing...')
                    # Try /? instead of --help, and accept return codes 0 or 1
                    result = subprocess.run([path, '/?'], capture_output=True, text=True, timeout=5)
                    if result.returncode in [0, 1]:  # Both are valid for help
                        iscc_found = path
                        print(f'    Found ISCC at: {path}')
                        break
                    else:
                        print(f'    File exists but test failed (return code: {result.returncode})')
                else:
                    print(f'    File does not exist')
        except (subprocess.CalledProcessError, FileNotFoundError, subprocess.TimeoutExpired) as e:
            print(f'    Error: {type(e).__name__}')
            continue
    
    if iscc_found:
        try:
            print('Running Inno Setup Compiler...')
            subprocess.check_call([iscc_found, iss_path])
            print('Inno Setup installer created.')
            # Check for installer .exe
            version = get_version()
            installer_name = f'AdvancedPDFReader-Setup-v{version}.exe'
            installer_path = os.path.join(PUBLIC_DIR, installer_name)
            if os.path.exists(installer_path):
                print(f'Installer created at {installer_path}')
            else:
                print(f'Installer .exe not found in {PUBLIC_DIR}. Please check Inno Setup output.')
        except Exception as e:
            print(f'Error running Inno Setup Compiler: {e}')
            print('You can manually compile the installer with Inno Setup using the script:', iss_path)
            print('Open installer.iss in Inno Setup and click "Compile". The output will be in /public.')
    else:
        print('Inno Setup Compiler (ISCC.exe) not found.')
        print('Please install Inno Setup from: https://jrsoftware.org/isinfo.php')
        print('Or add ISCC.exe to your PATH environment variable.')
        print('You can manually compile the installer with Inno Setup using the script:', iss_path)
        print('Open installer.iss in Inno Setup and click "Compile". The output will be in /public.')

def show_about(self):
    about_win = tk.Toplevel(self)
    about_win.title("About")
    about_win.configure(bg=BG_COLOR)
    about_win.resizable(False, False)
    # Load photo
    try:
        import tkinter as tk
        from tkinter import ttk
        from tkinter import PhotoImage
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
    exe_path = build_exe()
    version = get_version()
    iss_path = create_inno_setup_script(exe_path, version)
    run_inno_setup(iss_path)

if __name__ == '__main__':
    main() 