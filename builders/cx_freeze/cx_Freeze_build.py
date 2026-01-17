import os
import sys
import shutil
import subprocess
import time
from cx_Freeze import setup, Executable

# Change working directory to project root
os.chdir(os.path.join(os.path.dirname(__file__), '..', '..'))

APP_NAME = "Advanced PDF Reader"
VERSION = "3.0.0"
MAIN_SCRIPT = "pdfReader.py"
ICON_PATH = os.path.join("assets", "icons", "icon.ico")

include_files = [
    ("assets", "assets"),
    (os.path.join("builders", "cx_freeze", "sitecustomize.py"), "sitecustomize.py"),
    (os.path.join("builders", "cx_freeze", "sitecustomize.py"), "lib/sitecustomize.py"),
]

packages = [
    "tkinter",
    "PIL",
    "fitz",
    "requests",
    "pystray",
    "cryptography",
    "ctypes",
    "threading",
    "socket",
    "json",
    "encodings",
]

# Include pywin32 DLLs if present
try:
    import pywin32_system32
    pywin32_dir = os.path.dirname(pywin32_system32.__file__)
    for dll_name in ("pywintypes314.dll", "pythoncom314.dll"):
        dll_path = os.path.join(pywin32_dir, dll_name)
        if os.path.exists(dll_path):
            include_files.append((dll_path, dll_name))
except Exception:
    # Fallback search in site-packages
    candidate_dirs = [
        os.path.join(sys.prefix, "Lib", "site-packages", "pywin32_system32"),
        os.path.join(os.path.expanduser("~"), "AppData", "Roaming", "Python", f"Python{sys.version_info.major}{sys.version_info.minor}", "site-packages", "pywin32_system32"),
    ]
    for d in candidate_dirs:
        for dll_name in ("pywintypes314.dll", "pythoncom314.dll"):
            dll_path = os.path.join(d, dll_name)
            if os.path.exists(dll_path):
                include_files.append((dll_path, dll_name))

encodings_path = os.path.join(sys.base_prefix, "Lib", "encodings")
if os.path.isdir(encodings_path):
    include_files.append((encodings_path, "lib/encodings"))
    include_files.append((encodings_path, "encodings"))

def _safe_rmtree(path, retries=3, delay=0.5):
    for _ in range(retries):
        try:
            if os.path.exists(path):
                shutil.rmtree(path)
            return True
        except Exception:
            time.sleep(delay)
    return False

def _get_build_exe_dir():
    # Defined output directory
    base_dir = os.path.join("dist", "cx_freeze")
    
    # Try to clean existing dir
    if _safe_rmtree(base_dir):
        return base_dir
    elif not os.path.exists(base_dir):
        os.makedirs(base_dir, exist_ok=True)
        return base_dir
        
    print(f"Warning: Could not clean {base_dir}, using it as is.")
    return base_dir

def _update_installer_paths(build_exe_dir):
    iss_path = os.path.join("builders", "cx_freeze", "installer.iss")
    if not os.path.exists(iss_path):
        return
    build_glob = os.path.join(build_exe_dir, "*").replace("/", "\\")
    with open(iss_path, "r", encoding="utf-8") as f:
        lines = f.readlines()
    new_lines = []
    in_files_section = False
    files_inserted = False
    for line in lines:
        if line.strip().lower() == "[files]":
            in_files_section = True
            new_lines.append(line)
            continue
        if in_files_section:
            if line.startswith("["):
                if not files_inserted:
                    new_lines.append(
                        f'Source: "{build_glob}"; DestDir: "{{app}}"; Flags: ignoreversion recursesubdirs createallsubdirs\n'
                    )
                    files_inserted = True
                in_files_section = False
                new_lines.append(line)
                continue
            # Skip existing Source lines in [Files]
            if line.strip().startswith("Source:"):
                continue
            # Skip empty lines inside [Files]
            if not line.strip():
                continue
            new_lines.append(line)
            continue
        new_lines.append(line)
    if in_files_section and not files_inserted:
        new_lines.append(
            f'Source: "{build_glob}"; DestDir: "{{app}}"; Flags: ignoreversion recursesubdirs createallsubdirs\n'
        )
    with open(iss_path, "w", encoding="utf-8") as f:
        f.writelines(new_lines)

BUILD_EXE_DIR = _get_build_exe_dir()
_update_installer_paths(BUILD_EXE_DIR)

build_exe_options = {
    "packages": packages,
    "include_files": include_files,
    "include_msvcr": True,
    "includes": [
        "encodings",
    ],
    "excludes": [
        "pyarrow",
        "scipy",
        "pyodide",
        "pyu2f",
        "setuptools._vendor.packaging",
        "simplejson",
        "socks",
        "win_inet_pton",
        "zstandard.backend_rust",
    ],
    "build_exe": BUILD_EXE_DIR,
}

base = "gui" if sys.platform == "win32" else None

executables = [
    Executable(
        MAIN_SCRIPT,
        base=base,
        target_name=f"{APP_NAME}.exe" if sys.platform == "win32" else APP_NAME,
        icon=ICON_PATH if os.path.exists(ICON_PATH) else None,
    )
]

setup(
    name=APP_NAME,
    version=VERSION,
    description="Professional PDF Reader",
    options={"build_exe": build_exe_options},
    executables=executables,
)

# Build installer with Inno Setup (single EXE) after successful build
if sys.platform == "win32":
    try:
        # Use ISCC if available in PATH or common install paths
        iscc_path = os.environ.get("ISCC_PATH")
        if not iscc_path:
            candidates = [
                r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
                r"C:\Program Files\Inno Setup 6\ISCC.exe",
            ]
            for c in candidates:
                if os.path.exists(c):
                    iscc_path = c
                    break
        if iscc_path:
            subprocess.run([iscc_path, os.path.join("builders", "cx_freeze", "installer.iss")], check=True)
        else:
            subprocess.run(["iscc", os.path.join("builders", "cx_freeze", "installer.iss")], check=True)
    except Exception as e:
        print(f"[WARNING] Inno Setup build skipped or failed: {e}")
