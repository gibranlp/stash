#!/bin/sh
set -e

REPO="gibranlp/stash"
BIN="stash"

# Detect OS and architecture
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64) ASSET="stash-linux-x86_64"; INSTALL_DIR="/usr/bin" ;;
      *)
        echo "Unsupported architecture: $ARCH"
        echo "Only x86_64 Linux is supported at this time."
        exit 1
        ;;
    esac
    ;;
  Darwin)
    # /usr/bin is read-only on macOS (System Integrity Protection).
    INSTALL_DIR="/usr/local/bin"
    case "$ARCH" in
      x86_64)  ASSET="stash-macos-x86_64" ;;
      arm64)   ASSET="stash-macos-arm64" ;;
      *)
        echo "Unsupported architecture: $ARCH"
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS"
    echo "For Windows, run:"
    echo "  irm https://raw.githubusercontent.com/${REPO}/main/install.ps1 | iex"
    exit 1
    ;;
esac

# Get the latest release tag from GitHub API
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' \
  | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST" ]; then
  echo "Could not determine the latest release. Check your internet connection."
  exit 1
fi

URL="https://github.com/${REPO}/releases/download/${LATEST}/${ASSET}"

echo "Installing stash ${LATEST} for ${OS}/${ARCH} to ${INSTALL_DIR}..."

# Installing system-wide needs administrator rights unless already root.
if [ "$(id -u)" -eq 0 ]; then
  SUDO=""
elif command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
elif command -v doas >/dev/null 2>&1; then
  SUDO="doas"
else
  echo "Installing to ${INSTALL_DIR} needs root, but neither sudo nor doas is available."
  exit 1
fi

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT
curl -fsSL "$URL" -o "$TMP"

# Copy beside the target, then rename over it, so a running Stash is unaffected.
$SUDO mkdir -p "$INSTALL_DIR"
$SUDO cp "$TMP" "${INSTALL_DIR}/${BIN}.new"
$SUDO chmod 755 "${INSTALL_DIR}/${BIN}.new"
$SUDO mv -f "${INSTALL_DIR}/${BIN}.new" "${INSTALL_DIR}/${BIN}"

echo ""
echo "Installed to ${INSTALL_DIR}/${BIN}"

# Older versions installed to ~/.local/bin, which would shadow the new copy.
OLD="${HOME}/.local/bin/${BIN}"
if [ -f "$OLD" ] && [ "$OLD" != "${INSTALL_DIR}/${BIN}" ]; then
  rm -f "$OLD"
  echo "Removed the previous install at ${OLD}"
fi
