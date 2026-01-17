#!/bin/bash
set -e
cd "$(dirname "$0")/../.."

# Define directories
RESOURCE_DIR="builders/appimage"
BUILD_OUTPUT_DIR="dist/appimage"
APPDIR="$BUILD_OUTPUT_DIR/AppDir"
CX_BUILD_DIR="dist/cx_freeze"

# Ensure venv is used
export PATH="$(pwd)/venv/bin:$PATH"

# Clean up previous build artifacts
rm -rf "$BUILD_OUTPUT_DIR"
mkdir -p "$BUILD_OUTPUT_DIR"

# Step 1: Build with cx_Freeze (using venv)
echo "[INFO] Building with cx_Freeze..."
python3 "builders/cx_freeze/cx_Freeze_build.py" build

# Verify build exist
if [ ! -d "$CX_BUILD_DIR" ]; then
    echo "[ERROR] cx_Freeze build failed. Directory $CX_BUILD_DIR not found."
    exit 1
fi

# Step 2: Prepare AppDir
echo "[INFO] Preparing AppDir..."
mkdir -p "$APPDIR/usr/bin"

# Copy cx_Freeze output to AppDir/usr/bin
# We copy the CONTENTS of the build dir so 'Advanced PDF Reader' executable is in usr/bin
cp -r "$CX_BUILD_DIR/"* "$APPDIR/usr/bin/"

# Ensure executable permissions
chmod +x "$APPDIR/usr/bin/Advanced PDF Reader"

# Rename to match desktop file
mv "$APPDIR/usr/bin/Advanced PDF Reader" "$APPDIR/usr/bin/pdf-reader"

# Step 3: Run linuxdeploy (Prepare AppDir only)
# Download linuxdeploy if missing
if [ ! -f "$RESOURCE_DIR/linuxdeploy-x86_64.AppImage" ]; then
    wget -O "$RESOURCE_DIR/linuxdeploy-x86_64.AppImage" https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
    chmod +x "$RESOURCE_DIR/linuxdeploy-x86_64.AppImage"
fi

echo "[INFO] Running linuxdeploy to setup desktop integration..."
# We allow linuxdeploy to process desktop file and icons.
# We DO NOT pass --output appimage to prevent it from packaging prematurely.

"$RESOURCE_DIR/linuxdeploy-x86_64.AppImage" \
    --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/pdf-reader" \
    --icon-file assets/icons/pdf-reader.png \
    --desktop-file "$RESOURCE_DIR/pdf-reader.desktop"

# Step 4: Fix RPATH
# linuxdeploy changes RPATH to $ORIGIN/../lib, but cx_Freeze expects $ORIGIN/lib
echo "[INFO] Fixing RPATH..."
patchelf --set-rpath '$ORIGIN/lib' "$APPDIR/usr/bin/pdf-reader"

# Step 4b: Create custom AppRun to ensure correct runtime path
# A symlink confuses cx_Freeze (it thinks it's running from AppImage file path).
# We force execution from inside the mount.
rm -f "$APPDIR/AppRun"
cat > "$APPDIR/AppRun" << 'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
exec "${HERE}/usr/bin/pdf-reader" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# Step 5: Package manually with appimagetool
# Download appimagetool if missing
if [ ! -f "$RESOURCE_DIR/appimagetool-x86_64.AppImage" ]; then
    wget -O "$RESOURCE_DIR/appimagetool-x86_64.AppImage" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$RESOURCE_DIR/appimagetool-x86_64.AppImage"
fi

echo "[INFO] Packaging with appimagetool..."
ARCH=x86_64 "$RESOURCE_DIR/appimagetool-x86_64.AppImage" "$APPDIR" "$BUILD_OUTPUT_DIR/PDF_Reader-x86_64.AppImage"

echo "[SUCCESS] AppImage created at $BUILD_OUTPUT_DIR/PDF_Reader-x86_64.AppImage"
