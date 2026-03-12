#!/bin/sh
set -eu

VERSION=""
BASE_URL=""
OUTPUT=""
SHA_MACOS_ARM64=""
SHA_MACOS_X64=""
SHA_LINUX_X64=""

usage() {
    cat <<'EOF'
Generate a Homebrew formula for a released yacli version.

Usage:
  generate-homebrew-formula.sh \
    --version VERSION \
    --base-url URL \
    --output PATH \
    [--sha256-macos-arm64 SHA256] \
    [--sha256-macos-x64 SHA256] \
    [--sha256-linux-x64 SHA256]
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --base-url)
            BASE_URL="$2"
            shift 2
            ;;
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --sha256-macos-arm64)
            SHA_MACOS_ARM64="$2"
            shift 2
            ;;
        --sha256-macos-x64)
            SHA_MACOS_X64="$2"
            shift 2
            ;;
        --sha256-linux-x64)
            SHA_LINUX_X64="$2"
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

if [ -z "$VERSION" ] || [ -z "$BASE_URL" ] || [ -z "$OUTPUT" ]; then
    usage >&2
    exit 1
fi

if [ -z "$SHA_MACOS_ARM64" ] && [ -z "$SHA_MACOS_X64" ] && [ -z "$SHA_LINUX_X64" ]; then
    echo "At least one platform checksum is required." >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"

{
    cat <<EOF
class Yacli < Formula
  desc "Yandex Mail, Calendar, and Disk CLI for humans and AI agents"
  homepage "https://github.com/NextStat/yacli"
  version "${VERSION}"
  license "MIT"

EOF

    if [ -n "$SHA_MACOS_ARM64" ] || [ -n "$SHA_MACOS_X64" ]; then
        cat <<'EOF'
  on_macos do
EOF
        if [ -n "$SHA_MACOS_ARM64" ] && [ -n "$SHA_MACOS_X64" ]; then
            cat <<EOF
    if Hardware::CPU.arm?
      url "${BASE_URL}/yacli-aarch64-apple-darwin.tar.gz"
      sha256 "${SHA_MACOS_ARM64}"
    else
      url "${BASE_URL}/yacli-x86_64-apple-darwin.tar.gz"
      sha256 "${SHA_MACOS_X64}"
    end

EOF
        elif [ -n "$SHA_MACOS_ARM64" ]; then
            cat <<EOF
    if Hardware::CPU.arm?
      url "${BASE_URL}/yacli-aarch64-apple-darwin.tar.gz"
      sha256 "${SHA_MACOS_ARM64}"
    else
      odie "yacli Homebrew packages are not published for macOS x86_64 yet."
    end

EOF
        else
            cat <<EOF
    if Hardware::CPU.intel?
      url "${BASE_URL}/yacli-x86_64-apple-darwin.tar.gz"
      sha256 "${SHA_MACOS_X64}"
    else
      odie "yacli Homebrew packages are not published for macOS arm64 yet."
    end

EOF
        fi
        cat <<'EOF'
  end

EOF
    fi

    if [ -n "$SHA_LINUX_X64" ]; then
        cat <<EOF
  on_linux do
    if Hardware::CPU.intel?
      url "${BASE_URL}/yacli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${SHA_LINUX_X64}"
    else
      odie "yacli Homebrew packages are not published for Linux arm64 yet. Use the install script or cargo install."
    end
  end

EOF
    else
        cat <<'EOF'
  on_linux do
    odie "yacli Homebrew packages are not published for Linux yet. Use install.sh or cargo install."
  end

EOF
    fi

    cat <<'EOF'
  def install
    bin.install "yacli"
    doc.install "README.md", "LICENSE"
  end

  test do
    output = shell_output("#{bin}/yacli --format json guide --topic disk")
    assert_match "\"operation\":\"guide.show\"", output
  end
end
EOF
} > "$OUTPUT"
