# Icon Installation Guide

This document explains how to install icons for BittSeeder on different platforms.

## Windows

Icons are automatically embedded in the `.exe` file during build. No additional installation required.

To build:
```bash
cargo build --release
```

The resulting `bittseeder.exe` will have the icon embedded.

## macOS

### Using cargo-bundle (Recommended)

1. Install cargo-bundle:
```bash
cargo install cargo-bundle
```

2. Build and bundle:
```bash
cargo bundle --release
```

This creates a macOS `.app` bundle at:
`target/release/bundle/macos/BittSeeder.app`

3. Convert icon to .icns format (required for macOS):
```bash
# Using iconutil (macOS)
mkdir BittSeeder.iconset
sips -z 16 16 icon.ico --out BittSeeder.iconset/icon_16x16.png
sips -z 32 32 icon.ico --out BittSeeder.iconset/icon_16x16@2x.png
sips -z 32 32 icon.ico --out BittSeeder.iconset/icon_32x32.png
sips -z 64 64 icon.ico --out BittSeeder.iconset/icon_32x32@2x.png
sips -z 128 128 icon.ico --out BittSeeder.iconset/icon_128x128.png
sips -z 256 256 icon.ico --out BittSeeder.iconset/icon_128x128@2x.png
sips -z 256 256 icon.ico --out BittSeeder.iconset/icon_256x256.png
sips -z 512 512 icon.ico --out BittSeeder.iconset/icon_256x256@2x.png
sips -z 512 512 icon.ico --out BittSeeder.iconset/icon_512x512.png
sips -z 1024 1024 icon.ico --out BittSeeder.iconset/icon_512x512@2x.png
iconutil -c icns BittSeeder.iconset
```

4. Update `Cargo.toml` to use the `.icns` file:
```toml
[package.metadata.bundle]
icon = ["BittSeeder.icns"]  # Change from icon.ico
```

## Linux

### Option 1: Using .desktop file (Recommended)

1. Install the binary:
```bash
sudo cp target/release/bittseeder /usr/local/bin/
```

2. Install the desktop file:
```bash
sudo cp bittseeder.desktop /usr/share/applications/
```

3. Install the icon (convert .ico to .png first):
```bash
# Convert icon to PNG (requires ImageMagick)
convert icon.ico -resize 256x256 bittseeder.png
sudo cp bittseeder.png /usr/share/icons/hicolor/256x256/apps/
sudo cp bittseeder.png /usr/share/pixmaps/
```

4. Update desktop database:
```bash
sudo update-desktop-database /usr/share/applications/
```

### Option 2: AppImage (Self-contained)

1. Install cargo-appimage:
```bash
cargo install cargo-appimage
```

2. Build AppImage:
```bash
cargo appimage
```

This creates a standalone `.AppImage` file with embedded icons.

## Notes

- **Windows**: Icons are embedded at compile-time
- **macOS**: Requires .icns format and app bundles
- **Linux**: Uses .desktop files + PNG icons in system directories
