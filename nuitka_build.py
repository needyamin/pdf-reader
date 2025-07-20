import os
import sys
import shutil
from pathlib import Path
import subprocess
import json

# Configuration
APP_NAME = "Advanced PDF Reader"
VERSION = "1.0.0"
MAIN_SCRIPT = "pdfReader.py"
ICON_PATH = "assets/icons/icon.ico"
AUTHOR = "Yamin Hossain"
DESCRIPTION = "Modern PDF Reader and Annotator"

# Dependencies
REQUIRED_PACKAGES = [
    "nuitka",
    "ordered-set",
    "zstandard",
    "Pillow",
    "pymupdf",
    "requests"
]

# Build directory setup
BUILD_DIR = "build"
DIST_DIR = "dist"
ASSETS_DIR = "assets"

def clean_directories():
    """Clean build and dist directories"""
    print("Cleaning build directories...")
    for dir_path in [BUILD_DIR, DIST_DIR]:
        if os.path.exists(dir_path):
            shutil.rmtree(dir_path)
            print(f"Cleaned {dir_path}/")

def copy_assets():
    """Copy assets to build directory"""
    print("Copying assets...")
    assets_dest = os.path.join(DIST_DIR, "assets")
    if os.path.exists(ASSETS_DIR):
        shutil.copytree(ASSETS_DIR, assets_dest)
        print(f"Copied assets to {assets_dest}/")

def optimize_images():
    """Optimize image assets using PIL"""
    try:
        from PIL import Image
        print("Optimizing images...")
        for root, _, files in os.walk(os.path.join(DIST_DIR, "assets")):
            for file in files:
                if file.lower().endswith(('.png', '.jpg', '.jpeg')):
                    file_path = os.path.join(root, file)
                    try:
                        image = Image.open(file_path)
                        image.save(file_path, optimize=True, quality=85)
                        print(f"Optimized {file}")
                    except Exception as e:
                        print(f"Failed to optimize {file}: {e}")
    except ImportError:
        print("PIL not available, skipping image optimization")

def create_version_info():
    """Create version info for Windows executable"""
    version_info = f"""
VSVersionInfo(
  ffi=FixedFileInfo(
    filevers=({VERSION.replace('.', ', ')}, 0),
    prodvers=({VERSION.replace('.', ', ')}, 0),
    mask=0x3f,
    flags=0x0,
    OS=0x40004,
    fileType=0x1,
    subtype=0x0,
    date=(0, 0)
  ),
  kids=[
    StringFileInfo([
      StringTable(
        u'040904B0',
        [StringStruct(u'CompanyName', u'{AUTHOR}'),
         StringStruct(u'FileDescription', u'{DESCRIPTION}'),
         StringStruct(u'FileVersion', u'{VERSION}'),
         StringStruct(u'InternalName', u'{APP_NAME}'),
         StringStruct(u'LegalCopyright', u'© {AUTHOR}'),
         StringStruct(u'OriginalFilename', u'{APP_NAME}.exe'),
         StringStruct(u'ProductName', u'{APP_NAME}'),
         StringStruct(u'ProductVersion', u'{VERSION}')])
    ]),
    VarFileInfo([VarStruct(u'Translation', [1033, 1200])])
  ]
)
"""
    with open("version_info.txt", "w") as f:
        f.write(version_info)
    return "version_info.txt"

def check_dependencies():
    """Check and install required packages"""
    print("Checking dependencies...")
    try:
        import pkg_resources
        packages_to_install = []
        for package in REQUIRED_PACKAGES:
            try:
                pkg_resources.require(package)
            except pkg_resources.DistributionNotFound:
                packages_to_install.append(package)
        
        if packages_to_install:
            print(f"Installing missing packages: {', '.join(packages_to_install)}")
            subprocess.check_call([sys.executable, "-m", "pip", "install", *packages_to_install])
    except Exception as e:
        print(f"Error checking dependencies: {e}")
        sys.exit(1)

def build_executable():
    """Build executable using Nuitka"""
    print("Building executable with Nuitka...")
    
    nuitka_args = [
        sys.executable,
        "-m", "nuitka",
        "--mingw64",
        "--windows-company-name=" + AUTHOR,
        "--windows-product-name=" + APP_NAME,
        "--windows-file-version=" + VERSION,
        "--windows-product-version=" + VERSION,
        "--windows-file-description=" + DESCRIPTION,
        "--windows-icon-from-ico=" + ICON_PATH,
        "--disable-console",
        "--follow-imports",
        "--plugin-enable=tk-inter",
        "--include-package=PIL",
        "--include-package=fitz",
        "--include-package=requests",
        "--include-data-dir=assets=assets",
        "--output-dir=" + DIST_DIR,
        "--verbose",
        "--standalone",
        "--onefile",
        "--jobs=4",
        "--lto=yes",
        "--include-module=tkinter",
        "--include-module=tkinter.ttk",
        "--include-module=tkinter.messagebox",
        "--include-module=tkinter.filedialog",
        MAIN_SCRIPT
    ]
    
    try:
        print("Running Nuitka compilation...")
        result = subprocess.run(nuitka_args, check=True, capture_output=True, text=True)
        print(result.stdout)
        if result.stderr:
            print("Warnings/Errors:")
            print(result.stderr)
        print("Build completed successfully!")
    except subprocess.CalledProcessError as e:
        print(f"Build failed with error code {e.returncode}")
        print("Error output:")
        print(e.stdout)
        print(e.stderr)
        sys.exit(1)

def create_installer():
    """Create Windows installer using Inno Setup"""
    print("Creating installer...")
    inno_script = f"""
#define MyAppName "{APP_NAME}"
#define MyAppVersion "{VERSION}"
#define MyAppPublisher "{AUTHOR}"
#define MyAppExeName "{APP_NAME}.exe"

[Setup]
AppId={{{{YOUR-UUID-HERE}}}}
AppName={{#MyAppName}}
AppVersion={{#MyAppVersion}}
AppPublisher={{#MyAppPublisher}}
DefaultDirName={{autopf}}\\{{#MyAppName}}
DefaultGroupName={{#MyAppName}}
OutputDir=installer
OutputBaseFilename={{#MyAppName}}-Setup
Compression=lzma
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{{cm:CreateDesktopIcon}}"; GroupDescription: "{{cm:AdditionalIcons}}"
Name: "startmenuicon"; Description: "Create Start Menu shortcuts"; GroupDescription: "{{cm:AdditionalIcons}}"

[Files]
Source: "{DIST_DIR}\\{APP_NAME}.exe"; DestDir: "{{app}}"; Flags: ignoreversion
Source: "{DIST_DIR}\\assets\\*"; DestDir: "{{app}}\\assets\\"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{{group}}\\{{#MyAppName}}"; Filename: "{{app}}\\{{#MyAppExeName}}"
Name: "{{commondesktop}}\\{{#MyAppName}}"; Filename: "{{app}}\\{{#MyAppExeName}}"; Tasks: desktopicon
Name: "{{commonstartmenu}}\\{{#MyAppName}}"; Filename: "{{app}}\\{{#MyAppExeName}}"; Tasks: startmenuicon

[Run]
Filename: "{{app}}\\{{#MyAppExeName}}"; Description: "{{cm:LaunchProgram,{{#StringChange(MyAppName, '&', '&&')}}}}"; Flags: nowait postinstall skipifsilent
"""
    
    with open("installer.iss", "w") as f:
        f.write(inno_script)
    
    # Run Inno Setup Compiler if available
    try:
        subprocess.run(["iscc", "installer.iss"], check=True)
        print("Installer created successfully!")
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        print(f"Failed to create installer: {e}")
        print("Please install Inno Setup to create the installer.")

def main():
    """Main build process"""
    print(f"Building {APP_NAME} v{VERSION}")
    
    # Check dependencies first
    check_dependencies()
    
    # Clean previous builds
    clean_directories()
    
    # Build executable
    build_executable()
    
    # Copy and optimize assets
    copy_assets()
    optimize_images()
    
    # Create installer
    create_installer()
    
    print("\nBuild process completed!")
    print(f"Executable location: {DIST_DIR}/{APP_NAME}.exe")
    print("Installer location: installer/")

if __name__ == "__main__":
    main()