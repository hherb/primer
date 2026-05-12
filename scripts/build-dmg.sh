#!/usr/bin/env bash
set -euo pipefail

# Build a signed and notarized macOS DMG for primer-gui.
#
# Prerequisites:
#   - rustup-installed cargo at ~/.cargo/bin/cargo
#   - cargo-tauri CLI 2.x (`cargo install tauri-cli --version "^2.0"`)
#   - Developer ID Application: Horst Herb (X5DWXB4283) in login keychain
#   - APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH env vars
#
# Output: src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_*.dmg

if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    echo "error: ~/.cargo/bin/cargo not found; install via rustup" >&2
    exit 1
fi

if ! "$HOME/.cargo/bin/cargo" tauri --version >/dev/null 2>&1; then
    echo "error: cargo-tauri not installed" >&2
    echo "  install with: ~/.cargo/bin/cargo install tauri-cli --version '^2.0'" >&2
    exit 1
fi

if ! security find-identity -p codesigning -v 2>/dev/null \
    | grep -q "Developer ID Application: Horst Herb (X5DWXB4283)"; then
    echo "error: Developer ID Application cert not in login keychain" >&2
    echo "  create at developer.apple.com -> Certificates -> + -> Developer ID Application" >&2
    exit 1
fi

for var in APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH; do
    if [ -z "${!var:-}" ]; then
        echo "error: $var not set" >&2
        echo "  see README 'Building the macOS DMG' for setup" >&2
        exit 1
    fi
done

if [ ! -r "$APPLE_API_KEY_PATH" ]; then
    echo "error: APPLE_API_KEY_PATH not readable: $APPLE_API_KEY_PATH" >&2
    exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
gui_crate="$repo_root/src/crates/primer-gui"

cd "$gui_crate"
"$HOME/.cargo/bin/cargo" tauri build --bundles dmg --target aarch64-apple-darwin

dmg_path="$repo_root/src/target/aarch64-apple-darwin/release/bundle/dmg/Primer_0.1.0_aarch64.dmg"
if [ ! -f "$dmg_path" ]; then
    echo "error: expected DMG not produced at $dmg_path" >&2
    exit 1
fi

echo
echo "Built and notarized: $dmg_path"
echo "Test the install on another Mac before distributing."
