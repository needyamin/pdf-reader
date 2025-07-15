import os
import sys
import subprocess

REQUIREMENTS_TXT = 'requirements.txt'
MAIN_SCRIPT = 'pdfReader.py'
ICON = 'icon.ico'
EXE_NAME = 'PDFReader.exe'

REQUIREMENTS = [
    'PyMuPDF',
    'Pillow',
    'pywin32',
    'pystray',
]

def ensure_requirements_txt():
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
    if not os.path.exists(MAIN_SCRIPT):
        print(f'ERROR: {MAIN_SCRIPT} not found. Please make sure your main script is present.')
        return
    print('Building .exe with PyInstaller...')
    cmd = [
        sys.executable, '-m', 'PyInstaller',
        '--onefile', '--windowed',
        f'--icon={ICON}' if os.path.exists(ICON) else '',
        '--name', EXE_NAME.replace('.exe',''),
        MAIN_SCRIPT
    ]
    # Remove empty args
    cmd = [arg for arg in cmd if arg]
    subprocess.check_call(cmd)
    print(f'Build complete! Find your .exe in dist/{EXE_NAME}')

def main():
    ensure_requirements_txt()
    install_requirements()
    build_exe()

if __name__ == '__main__':
    main() 