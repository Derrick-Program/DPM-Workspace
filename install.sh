#!/usr/bin/env bash
# Installs the dpm CLI by downloading a prebuilt binary from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Derrick-Program/DPM-Workspace/main/install.sh | bash
#
# Env vars:
#   DPM_VERSION      release tag to install, e.g. "v0.1.5" (default: latest)
#   DPM_INSTALL_DIR  where to put the dpm binary (default: "$HOME/.local/bin")
set -euo pipefail

REPO="Derrick-Program/DPM-Workspace"
BIN_NAME="dpm"
INSTALL_DIR="${DPM_INSTALL_DIR:-$HOME/.local/bin}"

info() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$1" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-gnu" ;;
    *) die "unsupported OS: $os (dpm ships prebuilt binaries for macOS and Linux only)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) die "unsupported architecture: $arch" ;;
  esac
  printf '%s-%s\n' "$arch_part" "$os_part"
}

latest_version() {
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
}

main() {
  need_cmd curl
  need_cmd tar

  target="$(detect_target)"
  version="${DPM_VERSION:-$(latest_version)}"
  [ -n "$version" ] || die "could not determine latest dpm version (set DPM_VERSION to override)"
  case "$version" in
    v*) ;;
    *) version="v${version}" ;;
  esac

  tarball="${BIN_NAME}-${target}.tar.gz"
  url="https://github.com/${REPO}/releases/download/${version}/${tarball}"

  info "installing dpm ${version} (${target})"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  curl -fsSL "$url" -o "$tmpdir/$tarball" || die "download failed: $url"
  tar -xzf "$tmpdir/$tarball" -C "$tmpdir" || die "failed to extract $tarball"
  [ -f "$tmpdir/$BIN_NAME" ] || die "extracted archive did not contain a '$BIN_NAME' binary"

  mkdir -p "$INSTALL_DIR"
  mv "$tmpdir/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
  chmod +x "$INSTALL_DIR/$BIN_NAME"

  info "installed to $INSTALL_DIR/$BIN_NAME"

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      warn "$INSTALL_DIR is not on your PATH"
      printf '  add this to your shell profile (~/.bashrc, ~/.zshrc, ...):\n'
      printf '    export PATH="%s:$PATH"\n' "$INSTALL_DIR"
      ;;
  esac

  "$INSTALL_DIR/$BIN_NAME" --version || true
  info "run 'dpm --help' to get started"
}

main "$@"