#!/bin/sh
set -eu

VERSION=""
BASE_URL=""
OUTPUT_DIR=""
SHA_WINDOWS_X64=""

usage() {
    cat <<'EOF'
Generate a winget manifest bundle for a released yacli version.

Usage:
  generate-winget-manifests.sh \
    --version VERSION \
    --base-url URL \
    --output-dir DIR \
    --sha256-windows-x64 SHA256
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
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --sha256-windows-x64)
            SHA_WINDOWS_X64="$2"
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

if [ -z "$VERSION" ] || [ -z "$BASE_URL" ] || [ -z "$OUTPUT_DIR" ] || [ -z "$SHA_WINDOWS_X64" ]; then
    usage >&2
    exit 1
fi

package_id="NextStat.yacli"
manifest_root="${OUTPUT_DIR}/${package_id}"
mkdir -p "$manifest_root"

cat > "${manifest_root}/${package_id}.yaml" <<EOF
PackageIdentifier: ${package_id}
PackageVersion: ${VERSION}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
EOF

cat > "${manifest_root}/${package_id}.installer.yaml" <<EOF
PackageIdentifier: ${package_id}
PackageVersion: ${VERSION}
Platform:
  - Windows.Desktop
InstallModes:
  - interactive
  - silent
  - silentWithProgress
Installers:
  - Architecture: x64
    InstallerType: zip
    NestedInstallerType: portable
    NestedInstallerFiles:
      - RelativeFilePath: yacli.exe
        PortableCommandAlias: yacli
    InstallerUrl: ${BASE_URL}/yacli-x86_64-pc-windows-msvc.zip
    InstallerSha256: ${SHA_WINDOWS_X64}
ManifestType: installer
ManifestVersion: 1.6.0
EOF

cat > "${manifest_root}/${package_id}.locale.en-US.yaml" <<EOF
PackageIdentifier: ${package_id}
PackageVersion: ${VERSION}
PackageLocale: en-US
Publisher: NextStat
PublisherUrl: https://github.com/NextStat
PublisherSupportUrl: https://github.com/NextStat/yacli/issues
Author: NextStat
PackageName: yacli
PackageUrl: https://github.com/NextStat/yacli
License: MIT
LicenseUrl: https://github.com/NextStat/yacli/blob/v${VERSION}/LICENSE
ShortDescription: Yandex Mail, Calendar, and Disk CLI for humans and AI agents
Description: yacli is an open-source command-line interface for working with Yandex Mail, Calendar, and Disk from the terminal and from AI agents.
Moniker: yacli
Tags:
  - cli
  - yandex
  - mail
  - calendar
  - disk
ManifestType: defaultLocale
ManifestVersion: 1.6.0
EOF
