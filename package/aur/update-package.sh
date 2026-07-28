#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <package-directory> <version> <source-sha256>" >&2
  exit 2
fi

package_dir="$(cd "$1" && pwd)"
version="$2"
source_sha256="$3"
pkgbuild="${package_dir}/PKGBUILD"

if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "AUR publication accepts stable release versions only: ${version}" >&2
  exit 2
fi
if [[ ! "${source_sha256}" =~ ^[0-9a-f]{64}$ ]]; then
  echo "invalid SHA-256 digest: ${source_sha256}" >&2
  exit 2
fi

sed -i -E \
  -e "s/^pkgver=.*/pkgver=${version}/" \
  -e "s/^pkgrel=.*/pkgrel=1/" \
  -e "s/^sha256sums=.*/sha256sums=('${source_sha256}')/" \
  "${pkgbuild}"

(
  cd "${package_dir}"
  makepkg --printsrcinfo >.SRCINFO
)
