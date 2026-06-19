#!/usr/bin/env bash
set -euo pipefail

REPO="Rorical/RemoText"
BIN_NAME="remotext"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { printf "${CYAN}%s${NC}\n" "$*"; }
ok()    { printf "${GREEN}%s${NC}\n" "$*"; }
err()   { printf "${RED}%s${NC}\n" "$*" >&2; }

detect_platform() {
    local os arch
    case "$(uname -s)" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        *)
            err "Unsupported OS: $(uname -s)"
            exit 1
            ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)
            err "Unsupported architecture: $(uname -m)"
            exit 1
            ;;
    esac

    echo "${os}-${arch}"
}

get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        -H "Accept: application/vnd.github+json" \
        -H "X-GitHub-Api-Version: 2022-11-28" \
        2>/dev/null \
        | grep -o '"tag_name": *"[^"]*"' \
        | grep -o '"v[^"]*"' \
        | tr -d '"'
}

download_and_install() {
    local platform="$1"
    local version="$2"
    local asset="remotext-${platform}.tar.gz"
    local url="https://github.com/${REPO}/releases/download/${version}/${asset}"

    info "Downloading RemoText ${version} for ${platform}..."
    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    if ! curl -fsSLo "${tmpdir}/${asset}" "$url"; then
        err "Failed to download ${url}"
        exit 1
    fi

    info "Extracting..."
    tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"

    local install_dir
    if [ -w /usr/local/bin ]; then
        install_dir="/usr/local/bin"
    else
        install_dir="${HOME}/.local/bin"
        mkdir -p "$install_dir"
    fi

    install -m 755 "${tmpdir}/${BIN_NAME}" "${install_dir}/${BIN_NAME}"
    ok "Installed RemoText ${version} to ${install_dir}/${BIN_NAME}"

    if ! echo "$PATH" | grep -q "${install_dir}"; then
        info "Note: Add ${install_dir} to your PATH if not already present."
        info "  export PATH=\"${install_dir}:\$PATH\""
    fi
}

main() {
    info "RemoText one-click installer"

    if ! command -v curl &>/dev/null; then
        err "curl is required but not installed."
        exit 1
    fi

    local platform version
    platform="$(detect_platform)"
    info "Detected platform: ${platform}"

    version="$(get_latest_version)"
    if [ -z "$version" ]; then
        err "Could not determine latest version."
        exit 1
    fi
    info "Latest version: ${version}"

    download_and_install "$platform" "$version"

    info "Run 'remotext --help' to get started."
}

main
