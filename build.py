import os
import sys
import subprocess

REQUIREMENTS = [
    'PyMuPDF',
    'Pillow',
    'pywin32',
    'tk',
]

REQUIREMENTS_TXT = 'requirements.txt'
MAIN_SCRIPT = 'pdfReader.py'
ICON = 'icon.ico'
EXE_NAME = 'PDFReader.exe'

# Write requirements.txt if not present
if not os.path.exists(REQUIREMENTS_TXT):
    with open(REQUIREMENTS_TXT, 'w') as f:
        for pkg in REQUIREMENTS:
            f.write(pkg + '\n')
    print(f'Created {REQUIREMENTS_TXT}')

# Install requirements
print('Installing requirements...')
subprocess.check_call([sys.executable, '-m', 'pip', 'install', '-r', REQUIREMENTS_TXT])

# Build with PyInstaller
print('Building executable with PyInstaller...')
pyinstaller_args = [
    'pyinstaller',
    '--noconfirm',
    '--onefile',
    '--windowed',
    f'--name={EXE_NAME.replace(".exe", "")}',
]
if os.path.exists(ICON):
    pyinstaller_args.append(f'--icon={ICON}')
pyinstaller_args.append(MAIN_SCRIPT)

subprocess.check_call(pyinstaller_args)

print(f'Build complete! Find your .exe in the dist/ folder as {EXE_NAME}') 