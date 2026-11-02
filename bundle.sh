#!/bin/sh
# Builds Vault.app (release binary + icon). Usage: ./bundle.sh  then drag Vault.app to /Applications.
set -e
cd "$(dirname "$0")"

# The Dock icon is compiled into the binary from assets/icon.png, so render it before building.
cargo build --release --example render_icon
./target/release/examples/render_icon 512 assets/icon.png
cargo build --release --bin vault

# Icon: render every size macOS wants, then pack into AppIcon.icns.
ICONSET=target/AppIcon.iconset
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  ./target/release/examples/render_icon $s "$ICONSET/icon_${s}x${s}.png"
  ./target/release/examples/render_icon $((s * 2)) "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$ICONSET" -o target/AppIcon.icns

APP=Vault.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/vault "$APP/Contents/MacOS/vault"
cp target/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleName</key><string>Vault</string>
    <key>CFBundleDisplayName</key><string>Vault</string>
    <key>CFBundleExecutable</key><string>vault</string>
    <key>CFBundleIdentifier</key><string>com.vault.rust</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleVersion</key><string>1.0</string>
    <key>CFBundleShortVersionString</key><string>1.0</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST

# Ad-hoc sign so macOS runs it without complaints, and refresh Finder's icon cache for it.
codesign --force --deep --sign - "$APP"
touch "$APP"
echo "Built $(pwd)/$APP"
