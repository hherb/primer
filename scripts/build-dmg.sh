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

# Auto-source local notarization credentials if present. The env file
# itself is gitignored; see scripts/apple-notarize-env.sh.example for
# the format. CI workflows can skip this by either providing the env
# vars externally or by leaving the file absent.
#
# Sniff for placeholder values FIRST so a user who exports the trio
# directly in their shell isn't silently overridden by an unedited copy
# of the template. The "you@example.com" literal is distinctive enough
# to be a reliable indicator; once the user fills in real values it's
# gone from the file.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
env_file="$script_dir/apple-notarize-env.sh"
if [ -f "$env_file" ]; then
    if grep -q 'you@example.com' "$env_file"; then
        echo "warn: $env_file still has placeholder values; not sourcing it." >&2
        echo "      Edit it with real credentials, or export the trio in your shell." >&2
    else
        # shellcheck source=/dev/null
        source "$env_file"
    fi
fi

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

# Tauri's bundler accepts either of two notarization credential trios.
# We accept whichever is fully populated; if both or neither, we fail
# clearly. The placeholder Apple IDs in apple-notarize-env.sh.example
# ("you@example.com", "xxxx-xxxx-xxxx-xxxx", a real-looking but trivial
# team id) MUST be treated as not-set — otherwise Tauri picks them up
# and the notary submission fails several minutes later.
APPLE_ID_VAL="${APPLE_ID:-}"
APPLE_PASSWORD_VAL="${APPLE_PASSWORD:-}"
APPLE_TEAM_ID_VAL="${APPLE_TEAM_ID:-}"
if [ "$APPLE_ID_VAL" = "you@example.com" ] \
    || [ "$APPLE_PASSWORD_VAL" = "xxxx-xxxx-xxxx-xxxx" ]; then
    APPLE_ID_VAL=""; APPLE_PASSWORD_VAL=""
fi
APPLE_API_ISSUER_VAL="${APPLE_API_ISSUER:-}"
APPLE_API_KEY_VAL="${APPLE_API_KEY:-}"
APPLE_API_KEY_PATH_VAL="${APPLE_API_KEY_PATH:-}"
if [[ "$APPLE_API_ISSUER_VAL" == 00000000-* ]] \
    || [ "$APPLE_API_KEY_VAL" = "XXXXXXXXXX" ]; then
    APPLE_API_ISSUER_VAL=""; APPLE_API_KEY_VAL=""; APPLE_API_KEY_PATH_VAL=""
fi

path_a=0
if [ -n "$APPLE_ID_VAL" ] && [ -n "$APPLE_PASSWORD_VAL" ] && [ -n "$APPLE_TEAM_ID_VAL" ]; then
    path_a=1
fi
path_b=0
if [ -n "$APPLE_API_ISSUER_VAL" ] && [ -n "$APPLE_API_KEY_VAL" ] && [ -n "$APPLE_API_KEY_PATH_VAL" ]; then
    path_b=1
fi

if [ $((path_a + path_b)) -eq 0 ]; then
    echo "error: no notarization credentials configured" >&2
    echo "  fill in scripts/apple-notarize-env.sh from the .example template," >&2
    echo "  or export one of these trios in your shell:" >&2
    echo "    Path A: APPLE_ID, APPLE_PASSWORD (app-specific), APPLE_TEAM_ID" >&2
    echo "    Path B: APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH" >&2
    exit 1
fi

if [ $((path_a + path_b)) -eq 2 ]; then
    echo "error: both Path A and Path B notarization credentials are populated" >&2
    echo "  pick one and leave the other blank — Tauri prefers Path B when both" >&2
    echo "  are set, which can mask a stale Path A configuration" >&2
    exit 1
fi

if [ $path_b -eq 1 ] && [ ! -r "$APPLE_API_KEY_PATH_VAL" ]; then
    echo "error: APPLE_API_KEY_PATH not readable: $APPLE_API_KEY_PATH_VAL" >&2
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
