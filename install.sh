#!/usr/bin/env bash
set -euo pipefail

# Claw Code installer — downloads the latest release binary from GitHub.
# Usage:  curl -fsSL https://raw.githubusercontent.com/xiaoyu-work/copilot-code/main/install.sh | bash

REPO="xiaoyu-work/copilot-code"
INSTALL_DIR="${CLAW_INSTALL_DIR:-$HOME/.local/bin}"

detect_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  os="linux" ;;
    Darwin) os="macos" ;;
    *)      echo "Error: unsupported OS: $os" >&2; exit 1 ;;
  esac

  case "$arch" in
    x86_64|amd64)  arch="x64" ;;
    aarch64|arm64) arch="arm64" ;;
    *)             echo "Error: unsupported architecture: $arch" >&2; exit 1 ;;
  esac

  echo "claw-${os}-${arch}"
}

main() {
  local artifact tag url

  artifact="$(detect_platform)"

  # Get latest release tag
  tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)"

  if [ -z "$tag" ]; then
    echo "Error: could not find latest release. Make sure ${REPO} has a published release." >&2
    exit 1
  fi

  url="https://github.com/${REPO}/releases/download/${tag}/${artifact}"

  echo "Installing claw ${tag} (${artifact})..."
  mkdir -p "$INSTALL_DIR"
  curl -fSL --progress-bar "$url" -o "${INSTALL_DIR}/claw"
  chmod +x "${INSTALL_DIR}/claw"

  echo ""
  echo "✓ Installed to ${INSTALL_DIR}/claw"

  # Check if INSTALL_DIR is in PATH
  if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    echo ""
    echo "Add this to your shell profile:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
  fi

  echo ""
  echo "Get started:"
  echo "  claw login copilot    # authenticate with GitHub Copilot"
  echo "  claw --provider copilot"
}

main
