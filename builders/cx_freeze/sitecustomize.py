import os
import sys

# Ensure encodings package is discoverable for frozen builds
def _ensure_encodings_path():
    base_dir = os.path.dirname(sys.executable) if getattr(sys, "frozen", False) else os.getcwd()
    candidates = [
        os.path.join(base_dir, "lib", "encodings"),
        os.path.join(base_dir, "encodings"),
    ]
    for enc_path in candidates:
        if os.path.isdir(enc_path):
            lib_dir = os.path.dirname(enc_path)
            if lib_dir not in sys.path:
                sys.path.insert(0, lib_dir)
            break

_ensure_encodings_path()

