#!/bin/sh
# diffview installer
#
#   curl -fsSL https://raw.githubusercontent.com/icankeep/diffview/main/install.sh | sh
#
# Environment overrides:
#   DIFFVIEW_VERSION   release tag to install (default: latest)
#   DIFFVIEW_BIN_DIR   install directory (default: ~/.local/bin)
set -eu

REPO="icankeep/diffview"
BINARIES="diffview diffview-tui"
VERSION="${DIFFVIEW_VERSION:-latest}"
BIN_DIR="${DIFFVIEW_BIN_DIR:-$HOME/.local/bin}"

err() {
	printf 'error: %s\n' "$1" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || err "required command not found: $1"
}

need uname
need tar
need mktemp

# Pick a downloader.
if command -v curl >/dev/null 2>&1; then
	dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
	dl() { wget -qO "$2" "$1"; }
else
	err "need curl or wget to download"
fi

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
	Linux) os_part="unknown-linux-gnu" ;;
	Darwin) os_part="apple-darwin" ;;
	*) err "unsupported OS: $os" ;;
esac

case "$arch" in
	x86_64 | amd64) arch_part="x86_64" ;;
	arm64 | aarch64) arch_part="aarch64" ;;
	*) err "unsupported architecture: $arch" ;;
esac

target="${arch_part}-${os_part}"
asset="diffview-${target}.tar.gz"

if [ "$VERSION" = "latest" ]; then
	url="https://github.com/${REPO}/releases/latest/download/${asset}"
else
	url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'Downloading %s ...\n' "$url"
dl "$url" "$tmp/$asset" || err "download failed (no prebuilt binary for ${target}?)"

tar -xzf "$tmp/$asset" -C "$tmp" || err "failed to extract archive"

mkdir -p "$BIN_DIR"
for b in $BINARIES; do
	[ -f "$tmp/$b" ] || err "binary $b missing from archive"
	install -m 0755 "$tmp/$b" "$BIN_DIR/$b" 2>/dev/null || {
		cp "$tmp/$b" "$BIN_DIR/$b" && chmod 0755 "$BIN_DIR/$b"
	}
	printf 'Installed %s\n' "$BIN_DIR/$b"
done

# PATH hint.
case ":$PATH:" in
	*":$BIN_DIR:"*) ;;
	*)
		printf '\n%s is not on your PATH. Add it, e.g.:\n' "$BIN_DIR"
		printf '  export PATH="%s:$PATH"\n' "$BIN_DIR"
		;;
esac

# Auto-detect the terminal from the current environment and set it as the
# default (keeps any existing choice). Prints a bilingual summary of the
# chosen terminal and how to change it.
printf '\n'
"$BIN_DIR/diffview" config init || true

printf '\nrun / 运行: diffview .\n'
