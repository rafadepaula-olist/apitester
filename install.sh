#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
APP_DIR="$HOME/.local/share/applications"

GUI_BIN="$BIN_DIR/apitester"
CLI_BIN="$BIN_DIR/apitester-cli"
ICON_DEST="$ICON_DIR/apitester.svg"
DESKTOP_DEST="$APP_DIR/apitester.desktop"

refresh_desktop_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
    fi
}

if [[ "${1:-}" == "--uninstall" ]]; then
    echo "Uninstalling APITester from $HOME/.local..."
    rm -f "$GUI_BIN" "$CLI_BIN" "$ICON_DEST" "$DESKTOP_DEST"
    refresh_desktop_caches
    echo "APITester uninstalled."
    exit 0
fi

export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: cargo not found on PATH (checked \$HOME/.cargo/bin as well)." >&2
    echo "Install Rust from https://rustup.rs and re-run ./install.sh." >&2
    exit 1
fi

echo "Building APITester (release)..."
(cd "$SCRIPT_DIR" && cargo build --release --bin apitester --bin apitester-cli)

mkdir -p "$BIN_DIR" "$ICON_DIR" "$APP_DIR"

install -m 0755 "$SCRIPT_DIR/target/release/apitester" "$GUI_BIN"
install -m 0755 "$SCRIPT_DIR/target/release/apitester-cli" "$CLI_BIN"
install -m 0644 "$SCRIPT_DIR/assets/apitester.svg" "$ICON_DEST"

cat > "$DESKTOP_DEST" <<EOF
[Desktop Entry]
Type=Application
Name=APITester
Comment=Fast native Rust API client and load tester
Exec=$GUI_BIN
Icon=apitester
Terminal=false
Categories=Development;Network;
StartupNotify=true
StartupWMClass=apitester
EOF

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$DESKTOP_DEST"
fi

refresh_desktop_caches

echo "Installed:"
echo "  GUI:     $GUI_BIN"
echo "  CLI:     $CLI_BIN"
echo "  Icon:    $ICON_DEST"
echo "  Desktop: $DESKTOP_DEST"
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo "Note: Add $BIN_DIR to your PATH to run 'apitester' and 'apitester-cli' directly."
fi
