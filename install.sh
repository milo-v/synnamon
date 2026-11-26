#!/usr/bin/env bash
# Synnamon installer for Ubuntu/Debian (amd64).
#
#   curl -fsSL https://github.com/milo-v/synnamon/releases/latest/download/install.sh | sudo bash
#
# Downloads the .deb from the GitHub release, verifies its SHA-256 checksum, and installs it
# with apt. It does not touch PAM, sshd, or any authentication configuration.
#
# Environment:
#   SYNNAMON_VERSION  version to install, e.g. 0.1.0 (default: latest)
#   SYNNAMON_REPO     GitHub repo to download from (default: milo-v/synnamon)
set -euo pipefail

REPO="${SYNNAMON_REPO:-milo-v/synnamon}"
VERSION="${SYNNAMON_VERSION:-latest}"

die() { echo "error: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root, e.g.: curl -fsSL <url> | sudo bash"
command -v apt-get >/dev/null || die "apt-get not found; this installer supports Ubuntu/Debian only"
command -v curl >/dev/null || die "curl is required: apt-get install -y curl"
[ "$(dpkg --print-architecture)" = "amd64" ] || die "only amd64 packages are published (this machine: $(dpkg --print-architecture))"

if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/v${VERSION#v}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading synnamon ($VERSION)..."
curl -fsSL -o "$tmp/synnamon_amd64.deb" "$base/synnamon_amd64.deb" || die "download failed: $base/synnamon_amd64.deb"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"

echo "Verifying checksum..."
(cd "$tmp" && grep ' synnamon_amd64.deb$' SHA256SUMS | sha256sum -c -) || die "checksum mismatch; not installing"

echo "Installing..."
apt-get install -y "$tmp/synnamon_amd64.deb"
