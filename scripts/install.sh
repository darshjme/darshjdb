#!/usr/bin/env bash
# DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
# One-line installer: fetches the latest ddb-server release binary for your
# OS/arch from GitHub Releases and drops it at $DARSH_INSTALL_DIR (default
# $HOME/.darshjdb/bin/ddb).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/darshjme/darshjdb/main/scripts/install.sh | bash
#
# Environment:
#   DARSH_INSTALL_DIR   Override install directory (default: $HOME/.darshjdb/bin)

set -euo pipefail

REPO="darshjme/darshjdb"
INSTALL_DIR="${DARSH_INSTALL_DIR:-$HOME/.darshjdb/bin}"
mkdir -p "$INSTALL_DIR"

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64) ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *) echo "Unsupported arch: $ARCH" >&2; exit 1 ;;
esac

case "$OS" in
  linux|darwin) : ;;
  *) echo "Unsupported OS: $OS" >&2; exit 1 ;;
esac

LATEST=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' \
  | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "${LATEST:-}" ]; then
  echo "Failed to resolve latest release tag from GitHub API" >&2
  exit 1
fi

BINARY="ddb-server-${LATEST}-${OS}-${ARCH}"
URL="https://github.com/$REPO/releases/download/$LATEST/$BINARY"

echo "Installing DarshJDB $LATEST ($OS/$ARCH)..."
curl -fsSL "$URL" -o "$INSTALL_DIR/ddb"
chmod +x "$INSTALL_DIR/ddb"

echo "Done. Run: $INSTALL_DIR/ddb"
echo "DarshJDB — by Darshankumar Joshi (github.com/darshjme)"
