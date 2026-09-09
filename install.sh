#!/usr/bin/env bash
#
# rgfx installer
#
# Detects your OS/architecture, resolves the latest GitHub release, downloads the
# matching `rgfx-linux-<arch>.tar.gz`, verifies its SHA-256 checksum against the
# release `SHA256SUMS`, and installs the `rgfx` binary into ~/.local/bin.
#
# Usage:
#     curl -fsSL https://raw.githubusercontent.com/05vukasin/rgfx/main/install.sh | bash
#
# Environment overrides:
#     RGFX_REPO         owner/repo to install from   (default: 05vukasin/rgfx)
#     RGFX_INSTALL_DIR  install directory            (default: $HOME/.local/bin)
#     RGFX_VERSION      release tag to install       (default: latest)

set -euo pipefail

REPO="${RGFX_REPO:-05vukasin/rgfx}"
INSTALL_DIR="${RGFX_INSTALL_DIR:-$HOME/.local/bin}"

log() {
    printf '[rgfx] %s\n' "$1"
}

fail() {
    printf '[rgfx] error: %s\n' "$1" >&2
    exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

# --- detect platform --------------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux) OS_NAME="linux" ;;
    *) fail "Unsupported operating system: $OS (only Linux is supported)" ;;
esac

case "$ARCH" in
    x86_64 | amd64) ARCH_NAME="x86_64" ;;
    aarch64 | arm64) ARCH_NAME="aarch64" ;;
    *) fail "Unsupported architecture: $ARCH" ;;
esac

# --- resolve release version ------------------------------------------------

VERSION="${RGFX_VERSION:-}"

if [ -z "$VERSION" ]; then
    log "Detecting latest release..."
    # Follow the /releases/latest redirect and read the resolved tag from the URL.
    LATEST_URL="$(
        curl -fsSL \
            -o /dev/null \
            -w '%{url_effective}' \
            "https://github.com/${REPO}/releases/latest"
    )"
    VERSION="${LATEST_URL##*/}"
fi

[ -n "$VERSION" ] || fail "Could not determine the latest release"

ARCHIVE="rgfx-${OS_NAME}-${ARCH_NAME}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
ARCHIVE_URL="${BASE_URL}/${ARCHIVE}"
SUMS_URL="${BASE_URL}/SHA256SUMS"

# --- download ---------------------------------------------------------------

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

log "Downloading rgfx ${VERSION} (${ARCH_NAME})..."
curl -fL "$ARCHIVE_URL" -o "$TMP_DIR/$ARCHIVE" \
    || fail "Failed to download $ARCHIVE_URL"

log "Downloading checksums..."
curl -fL "$SUMS_URL" -o "$TMP_DIR/SHA256SUMS" \
    || fail "Failed to download $SUMS_URL"

# --- verify checksum --------------------------------------------------------

log "Verifying SHA-256 checksum..."

expected="$(awk -v name="$ARCHIVE" '$2 == name || $2 == "*"name { print $1 }' \
    "$TMP_DIR/SHA256SUMS" | head -n 1)"

[ -n "$expected" ] || fail "No checksum for $ARCHIVE in SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$TMP_DIR/$ARCHIVE" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$TMP_DIR/$ARCHIVE" | awk '{ print $1 }')"
else
    fail "Need sha256sum or shasum to verify the download"
fi

if [ "$expected" != "$actual" ]; then
    fail "Checksum mismatch for $ARCHIVE (expected $expected, got $actual)"
fi

log "Checksum OK."

# --- extract & install ------------------------------------------------------

log "Extracting..."
tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"

BINARY="$(find "$TMP_DIR" -type f -name rgfx -print -quit)"
[ -n "$BINARY" ] || fail "rgfx binary was not found in the archive"

mkdir -p "$INSTALL_DIR"
install -m755 "$BINARY" "$INSTALL_DIR/rgfx"

log "Installed rgfx ${VERSION} to $INSTALL_DIR/rgfx"

# --- PATH guidance ----------------------------------------------------------

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf '\n[rgfx] %s is not on your PATH.\n' "$INSTALL_DIR"
        printf '[rgfx] Add this to your shell configuration:\n\n'
        printf '    export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
        ;;
esac

log "Done. Try:"
printf '\n    rgfx --help\n    rgfx model.obj\n\n'
