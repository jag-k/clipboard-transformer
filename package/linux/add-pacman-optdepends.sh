#!/usr/bin/env bash
set -euo pipefail

pkgbuild="${1:-target/packager/PKGBUILD}"

if grep -q '^optdepends=' "${pkgbuild}"; then
  exit 0
fi

temporary="$(mktemp "${pkgbuild}.XXXXXX")"
trap 'rm -f "${temporary}"' EXIT

awk '
  /^provides=/ {
    print "optdepends=("
    print "  '\''wayland: native Wayland clipboard support'\''"
    print "  '\''xdg-utils: fallback for opening support links'\''"
    print ")"
  }
  { print }
' "${pkgbuild}" >"${temporary}"

mv "${temporary}" "${pkgbuild}"
trap - EXIT
