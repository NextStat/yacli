#!/bin/sh
set -eu

TARGET=""
BINARY=""
OUTPUT_DIR=""

usage() {
    cat <<'EOF'
Package a built yacli binary into a release archive.

Usage:
  package-release.sh --target TARGET --binary PATH --output-dir DIR
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --binary)
            BINARY="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
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

if [ -z "$TARGET" ] || [ -z "$BINARY" ] || [ -z "$OUTPUT_DIR" ]; then
    usage >&2
    exit 1
fi

if [ ! -f "$BINARY" ]; then
    echo "Binary not found: $BINARY" >&2
    exit 1
fi

asset_base="yacli-${TARGET}"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/yacli-package.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM HUP

stage_dir="${tmp_dir}/${asset_base}"
mkdir -p "$stage_dir" "$OUTPUT_DIR"
cp "$BINARY" "${stage_dir}/$(basename "$BINARY")"
cp README.md LICENSE "$stage_dir/"

archive_path="${OUTPUT_DIR}/${asset_base}.tar.gz"
tar -czf "$archive_path" -C "$tmp_dir" "$asset_base"
printf '%s\n' "$archive_path"
