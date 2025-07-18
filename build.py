import os
import sys
import subprocess
import re

REQUIREMENTS_TXT = 'requirements.txt'
MAIN_SCRIPT = 'pdfReader.py'
ICON = os.path.join('assets', 'icons', 'icon.ico')


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

def main():
    ensure_requirements_txt()
    install_requirements()
    build_exe()

if __name__ == '__main__':
    main() 