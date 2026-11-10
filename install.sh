#!/bin/sh
# Installs the latest `dato` release for this system.
#
#   curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/LeoBorai/dato/main/install.sh | sh
#
# Options, as environment variables:
#   DATO_VERSION      a release tag such as v0.1.0 (default: the latest release)
#   DATO_INSTALL_DIR  where to put the binary (default: $HOME/.local/bin)
#
# The download is checked against the SHA-256 published next to it. Nothing is installed if the
# check fails. Windows is not covered by this script: download the .zip from the releases page.

set -eu

REPO="LeoBorai/dato"

say() { printf 'dato-install: %s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found"; }

# Saves the URL's body to the file given as $2.
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget --https-only -q -O "$2" "$1"
    else
        die "'curl' or 'wget' is required"
    fi
}

target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$arch" in
        x86_64 | amd64) arch="x86_64" ;;
        aarch64 | arm64) arch="aarch64" ;;
        *) die "unsupported CPU architecture: $arch" ;;
    esac
    case "$os" in
        Linux)
            if ldd --version 2>&1 | grep -qi musl; then
                die "the Linux build needs glibc; this system uses musl (Alpine). Build from source: cargo install --git https://github.com/$REPO dato"
            fi
            echo "${arch}-unknown-linux-gnu"
            ;;
        Darwin) echo "${arch}-apple-darwin" ;;
        MINGW* | MSYS* | CYGWIN*) die "on Windows, download the .zip from https://github.com/$REPO/releases/latest" ;;
        *) die "unsupported operating system: $os" ;;
    esac
}

latest_tag() {
    # The /releases/latest page redirects to /releases/tag/<tag>; reading the redirect avoids
    # the GitHub API and its rate limit.
    if command -v curl >/dev/null 2>&1; then
        url="$(curl --proto '=https' --tlsv1.2 -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")" \
            || die "cannot find the latest release; is there one yet? Set DATO_VERSION to a tag"
        echo "${url##*/}"
    else
        die "finding the latest release needs 'curl'; set DATO_VERSION instead"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        die "'sha256sum' or 'shasum' is required to check the download"
    fi
}

main() {
    need uname
    need tar
    need mktemp

    triple="$(target)"
    tag="${DATO_VERSION:-$(latest_tag)}"
    case "$tag" in v[0-9]*) ;; *) die "unexpected release tag: '$tag'" ;; esac
    dest="${DATO_INSTALL_DIR:-$HOME/.local/bin}"

    name="dato-${tag}-${triple}"
    base="https://github.com/$REPO/releases/download/$tag"

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "downloading $name"
    fetch "$base/$name.tar.gz" "$tmp/$name.tar.gz" || die "cannot download $base/$name.tar.gz"
    fetch "$base/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256" || die "cannot download the checksum"

    expected="$(cut -d ' ' -f 1 "$tmp/$name.tar.gz.sha256")"
    actual="$(sha256_of "$tmp/$name.tar.gz")"
    [ -n "$expected" ] && [ "$expected" = "$actual" ] || die "checksum mismatch: expected '$expected', got '$actual'"

    tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
    [ -f "$tmp/$name/dato" ] || die "the archive has no 'dato' binary"

    mkdir -p "$dest"
    # Copy beside the target, then rename: an interrupted install never leaves a partial binary.
    cp "$tmp/$name/dato" "$dest/.dato.new"
    chmod 755 "$dest/.dato.new"
    mv -f "$dest/.dato.new" "$dest/dato"

    say "installed dato $tag to $dest/dato"
    case ":$PATH:" in
        *":$dest:"*) ;;
        *) say "add $dest to your PATH, for example: export PATH=\"$dest:\$PATH\"" ;;
    esac
}

# Everything runs from one call at the end, so a download cut short cannot run half a script.
main "$@"
