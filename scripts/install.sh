#!/bin/sh
set -eu

REPO_SLUG="NextStat/yacli"
VERSION="latest"
INSTALL_DIR="${HOME}/.local/bin"
BASE_URL=""

usage() {
    cat <<'EOF'
Install yacli from GitHub Releases.

Usage:
  install.sh [--version VERSION] [--install-dir DIR] [--base-url URL]

Options:
  --version VERSION      Specific version without leading v, or "latest" (default)
  --install-dir DIR      Destination directory for the yacli binary
  --base-url URL         Override download base URL that serves assets and SHA256SUMS
  --help                 Show this help message
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --install-dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        --base-url)
            BASE_URL="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

normalize_version() {
    value="$1"
    case "$value" in
        latest) printf '%s' "latest" ;;
        v*) printf '%s' "$value" ;;
        *) printf 'v%s' "$value" ;;
    esac
}

detect_asset() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) platform="apple-darwin" ;;
        Linux) platform="unknown-linux-gnu" ;;
        *)
            echo "Unsupported operating system: $os" >&2
            exit 1
            ;;
    esac

    case "$arch" in
        arm64|aarch64) cpu="aarch64" ;;
        x86_64|amd64) cpu="x86_64" ;;
        *)
            echo "Unsupported architecture: $arch" >&2
            exit 1
            ;;
    esac

    printf 'yacli-%s-%s.tar.gz' "$cpu" "$platform"
}

checksum_cmd() {
    if command -v shasum >/dev/null 2>&1; then
        printf '%s' "shasum -a 256"
    elif command -v sha256sum >/dev/null 2>&1; then
        printf '%s' "sha256sum"
    else
        echo "Need shasum or sha256sum to verify yacli release assets." >&2
        exit 1
    fi
}

asset_name="$(detect_asset)"
checksum_tool="$(checksum_cmd)"

if [ -z "$BASE_URL" ]; then
    normalized_version="$(normalize_version "$VERSION")"
    if [ "$normalized_version" = "latest" ]; then
        BASE_URL="https://github.com/${REPO_SLUG}/releases/latest/download"
    else
        BASE_URL="https://github.com/${REPO_SLUG}/releases/download/${normalized_version}"
    fi
fi

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/yacli-install.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM HUP

asset_path="${tmp_dir}/${asset_name}"
checksums_path="${tmp_dir}/SHA256SUMS"

curl -fsSL "${BASE_URL}/${asset_name}" -o "$asset_path"
curl -fsSL "${BASE_URL}/SHA256SUMS" -o "$checksums_path"

expected_sum="$(awk -v asset="$asset_name" '$2 == asset { print $1; exit }' "$checksums_path")"
if [ -z "$expected_sum" ]; then
    echo "Could not find checksum for ${asset_name} in SHA256SUMS" >&2
    exit 1
fi

actual_sum="$(eval "$checksum_tool \"\$asset_path\"" | awk '{print $1}')"
if [ "$actual_sum" != "$expected_sum" ]; then
    echo "Checksum mismatch for ${asset_name}" >&2
    echo "Expected: $expected_sum" >&2
    echo "Actual:   $actual_sum" >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR"
tar -xzf "$asset_path" -C "$tmp_dir"

binary_path="$(find "$tmp_dir" -type f -name yacli -perm -u+x | head -n 1)"
if [ -z "$binary_path" ]; then
    echo "Could not locate yacli binary in archive ${asset_name}" >&2
    exit 1
fi

cp "$binary_path" "${INSTALL_DIR}/yacli"
chmod +x "${INSTALL_DIR}/yacli"

echo "Installed yacli to ${INSTALL_DIR}/yacli"
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo "Add ${INSTALL_DIR} to PATH to use yacli directly."
        ;;
esac
