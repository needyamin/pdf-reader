import os
import sys
import shutil
from pathlib import Path
import subprocess
import json
import uuid

# Change working directory to project root
os.chdir(os.path.join(os.path.dirname(__file__), '..', '..'))

# ---
# To access bundled assets in your app, use this function:
# (Copy this to your main app, e.g., pdfReader.py)
def resource_path(relative_path):
    import sys, os
    # Prefer assets located next to the executable (installer layout)
    if getattr(sys, 'frozen', False):
        base_dir = os.path.dirname(sys.executable)
        candidate = os.path.join(base_dir, relative_path)
        if os.path.exists(candidate):
            return candidate
    # Nuitka/PyInstaller onefile temp extraction dir
    temp_dir = getattr(sys, '_MEIPASS', None) or os.environ.get('NUITKA_ONEFILE_TEMP_DIR')
    if temp_dir:
        candidate = os.path.join(temp_dir, relative_path)
        if os.path.exists(candidate):
            return candidate
    # Fallback to project directory (dev mode)
    return os.path.join(os.path.abspath('.'), relative_path)
# Example: resource_path('assets/icons/icon.ico')
# ---

# Production Configuration
APP_NAME = "Advanced PDF Reader"
VERSION = "3.0.0"
MAIN_SCRIPT = "pdfReader.py"
ICON_PATH = resource_path("assets/icons/icon.ico")
AUTHOR = "YAMiN HOSSAIN"
DESCRIPTION = "Professional PDF Reader"

# Dependencies
REQUIRED_PACKAGES = [
    "nuitka>=1.8.0",
    "ordered-set",
    "zstandard",
    "Pillow>=10.0.0",
    "pymupdf>=1.23.0",
    "requests>=2.31.0"
]

# Build directory setup
BUILD_DIR = "build"
DIST_DIR = os.path.join("dist", "nuitka")
ASSETS_DIR = "assets"

def clean_directories():
    """Clean build and dist directories"""
    print("Cleaning build directories...")
    # Clean output dir
    if os.path.exists(DIST_DIR):
        shutil.rmtree(DIST_DIR)
        print(f"  [REMOVED] {DIST_DIR}/")
    
    # Clean temp build dir
    if os.path.exists(BUILD_DIR):
        shutil.rmtree(BUILD_DIR)
        print(f"  [REMOVED] {BUILD_DIR}/")

def copy_assets(target_root: str = None):
    """Copy assets to the specified output directory (defaults to DIST_DIR)."""
    print("Preparing assets...")
    target_root = target_root or DIST_DIR
    assets_dest = os.path.join(target_root, "assets")

    if os.path.exists(assets_dest):
        shutil.rmtree(assets_dest)
        print(f"  [REMOVED] Existing assets at {assets_dest}/")

    if os.path.exists(ASSETS_DIR):
        print(f"  [COPYING] Assets from {ASSETS_DIR}/ to {assets_dest}/")
        shutil.copytree(ASSETS_DIR, assets_dest)
        print(f"  [SUCCESS] Assets copied successfully!")
    else:
        print(f"  [WARNING] Assets directory {ASSETS_DIR}/ not found")

    # Ensure session file exists next to the executable for persistence
    session_file = os.path.join(assets_dest, "json", "last_session.json")
    os.makedirs(os.path.dirname(session_file), exist_ok=True)
    if not os.path.exists(session_file):
        print(f"  Creating session file: {session_file}")
        with open(session_file, 'w') as f:
            json.dump({"file": "", "page": 0, "timestamp": 0}, f)
        print(f"  [SUCCESS] Session file created!")
    else:
        print(f"  [OK] Session file already exists")

    print("Assets prepared successfully!")

def optimize_images(target_root: str = None):
    """Optimize image assets using PIL in the specified output directory (defaults to DIST_DIR)."""
    try:
        from PIL import Image
        print("Optimizing images...")
        optimized_count = 0
        target_root = target_root or DIST_DIR
        for root, _, files in os.walk(os.path.join(target_root, "assets")):
            for file in files:
                if file.lower().endswith(('.png', '.jpg', '.jpeg')):
                    file_path = os.path.join(root, file)
                    try:
                        image = Image.open(file_path)
                        image.save(file_path, optimize=True, quality=85)
                        optimized_count += 1
                        print(f"  [OPTIMIZED] {file}")
                    except Exception as e:
                        print(f"  [WARNING] Failed to optimize {file}: {e}")
        if optimized_count > 0:
            print(f"  [SUCCESS] Optimized {optimized_count} images")
        else:
            print("  [OK] No images found to optimize")
    except ImportError:
        print("  [WARNING] PIL not available - skipping image optimization")

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

def find_iscc_executable():
    """Locate Inno Setup Compiler (ISCC.exe) on Windows.

    Order of detection:
      1) PATH (iscc / ISCC.exe)
      2) INNO_SETUP_PATH env var (file or directory)
      3) Common install directories in Program Files / Program Files (x86)
    """
    # PATH lookup
    path_exe = shutil.which("iscc") or shutil.which("ISCC.exe")
    if path_exe and os.path.exists(path_exe):
        return path_exe

    candidates = []

    # Environment variable can point to file or directory
    env_path = os.environ.get("INNO_SETUP_PATH")
    if env_path:
        candidates.append(env_path)
        if os.path.isdir(env_path):
            candidates.append(os.path.join(env_path, "ISCC.exe"))

    # Common installation locations
    program_files = os.environ.get("ProgramFiles")
    program_files_x86 = os.environ.get("ProgramFiles(x86)")

    defaults = [
        r"C:\\Program Files\\Inno Setup 6\\ISCC.exe",
        r"C:\\Program Files (x86)\\Inno Setup 6\\ISCC.exe",
    ]
    for base in (program_files, program_files_x86):
        if base:
            defaults.append(os.path.join(base, "Inno Setup 6", "ISCC.exe"))

    candidates.extend(defaults)

    for candidate in candidates:
        if candidate and os.path.exists(candidate):
            return candidate

    return None

def check_dependencies():
    """Check and install required packages"""
    print("Verifying build environment...")
    try:
        import importlib.metadata as metadata
        packages_to_install = []
        print("Checking required packages:")
        for package in REQUIRED_PACKAGES:
            package_name = package.split('>=')[0].split('==')[0]
            try:
                version = metadata.version(package_name)
                print(f"  [OK] {package_name} (v{version}) - installed")
            except metadata.PackageNotFoundError:
                print(f"  [MISSING] {package_name} - missing")
                packages_to_install.append(package)
        
        if packages_to_install:
            print(f"\nInstalling {len(packages_to_install)} missing packages...")
            for package in packages_to_install:
                print(f"  Installing: {package}")
            subprocess.check_call([sys.executable, "-m", "pip", "install", *packages_to_install])
            print("[SUCCESS] All packages installed successfully!")
        else:
            print("[OK] All required packages are already installed!")
    except Exception as e:
        print(f"[ERROR] Build environment error: {e}")
        sys.exit(1)

def build_executable():
    """Build production executable using Nuitka"""
    print("Starting production build...")
    
    # Compute a balanced parallel jobs count to avoid RAM pressure on Windows
    try:
        cpu_count = os.cpu_count() or 2
        jobs_count = max(2, min(4, cpu_count // 2))
    except Exception:
        jobs_count = 2

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
        # This flag ensures the app does not show a terminal window
        "--windows-console-mode=disable",
        "--plugin-enable=tk-inter",
        "--include-package=PIL",
        "--include-package=fitz",
        "--include-package=requests",
        # This bundles ALL assets (icons, images, json, etc.)
        "--include-data-dir=assets=assets",
        # Ensure the output filename matches APP_NAME.exe
        "--output-filename=" + APP_NAME + ".exe",
        "--output-dir=" + DIST_DIR,
        "--standalone",
        "--onefile",
        f"--jobs={jobs_count}",  # dynamic parallelism for faster builds
        "--onefile-no-compression",  # faster build/startup; larger exe
        "--lto=no",  # Disabled LTO to fix MinGW compilation issues
        "--assume-yes-for-downloads",
        # Additional MinGW compatibility options
        "--static-libpython=no",
        "--disable-dll-dependency-cache",
        "--no-prefer-source",
        # Production optimizations
        "--remove-output",
        "--no-pyi-file",
        "--warn-unusual-code",
        "--warn-implicit-exceptions",
        MAIN_SCRIPT
    ]
    
    try:
        print("Building production executable...")
        print("Nuitka build output:")
        print("-" * 50)
        result = subprocess.run(nuitka_args, check=True)
        print("-" * 50)
        print("Build completed successfully!")
        print(f"Executable location: {os.path.join(DIST_DIR, APP_NAME + '.exe')}")
    except subprocess.CalledProcessError as e:
        print(f"Build failed with error code {e.returncode}")
        print("Check the output above for error details.")
        sys.exit(1)

def create_installer():
    """Create Windows installer using Inno Setup"""
    print("Creating installer...")
    # Stable AppId based on app identity (remains constant across versions)
    app_guid = str(uuid.uuid5(uuid.NAMESPACE_URL, f"{APP_NAME}|{AUTHOR}"))
    # Use f-string with proper escaping for Inno Setup variables
    dist_path = os.path.join("public", DIST_DIR).replace("/", "\\")
    inno_script = f"""
#define MyAppName "{APP_NAME}"
#define MyAppVersion "{VERSION}"
#define MyAppPublisher "{AUTHOR}"
#define MyAppExeName "{APP_NAME}.exe"

[Setup]
; Use the generated GUID as a literal AppId to remain stable across installs
AppId={{{{{app_guid}}}}}
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
Source: "{dist_path}\\*"; DestDir: "{{app}}"; Flags: ignoreversion recursesubdirs createallsubdirs

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
    iscc_path = find_iscc_executable()
    if not iscc_path:
        print("Failed to create installer: Inno Setup Compiler (ISCC.exe) not found.")
        print("Searched PATH, INNO_SETUP_PATH, and common install directories.")
        print("If installed at a custom location, set INNO_SETUP_PATH to the directory or full path to ISCC.exe.")
        return False
    try:
        subprocess.run([iscc_path, "installer.iss"], check=True)
        print("Installer created successfully!")
        return True
    except subprocess.CalledProcessError as e:
        print(f"Failed to create installer (exit code {e.returncode}).")
        return False

def main():
    """Main build process"""
    print(f"Building {APP_NAME} v{VERSION}")
    print("=" * 60)
    
    try:
        # Check dependencies first
        print("\n[STEP 1] Checking dependencies...")
        check_dependencies()
        print("[SUCCESS] Dependencies check completed!")
        
        # Clean previous builds
        print("\n[STEP 2] Cleaning previous builds...")
        clean_directories()
        print("[SUCCESS] Cleanup completed!")
        
        # Build executable
        print("\n[STEP 3] Building executable with Nuitka...")
        build_executable()
        print("[SUCCESS] Executable build completed!")

        # The output is directly in DIST_DIR now.
        print(f"\n[STEP 4] Build output is in {DIST_DIR}")

        # Copy assets beside the executable for installer packaging and runtime access
        print(f"\n[STEP 5] Copying assets to {DIST_DIR}...")
        copy_assets(target_root=DIST_DIR)
        # Optional: optimize image assets to reduce installer size
        optimize_images(target_root=DIST_DIR)
        print(f"[SUCCESS] Assets prepared in {DIST_DIR}!")
        print("[SUCCESS] Assets prepared in public/dist!")
        
        # Create installer
        print("\n[STEP 6] Creating installer...")
        installer_ok = create_installer()
        if installer_ok:
            print("[SUCCESS] Installer creation completed!")
        else:
            print("[WARNING] Installer was not created. You can still run the portable EXE from public/.")
        
        print("\n" + "=" * 60)
        print("SUCCESS: Production build completed successfully!")
        print(f"Executable: {os.path.join(DIST_DIR, APP_NAME + '.exe')}")
        if installer_ok:
            print(f"Installer: installer/{APP_NAME}-Setup.exe")
            print("Ready for deployment!")
        else:
            print("Installer not built. Once ISCC is available, re-run to build the installer.")
        print("=" * 60)
        
    except Exception as e:
        print(f"\nERROR: Build failed with error: {e}")
        print("Please check the output above for details.")
        sys.exit(1)

if __name__ == "__main__":
    main()