#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
generator_commit="737c0085912f9f7dabf9341d4608e2a77a51a73a"
generator_sha256="b373c8ab1a05378ec5d8ed0645c7b127bcec7d2f7a1798694fbc627d570d856c"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/clipboard-transformer-flatpak.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

curl --fail --location --silent --show-error \
  "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${generator_commit}/cargo/flatpak-cargo-generator.py" \
  --output "$temporary/flatpak-cargo-generator.py"
printf '%s  %s\n' "$generator_sha256" "$temporary/flatpak-cargo-generator.py" |
  shasum -a 256 --check

uv run "$temporary/flatpak-cargo-generator.py" \
  "$root/Cargo.lock" \
  --output "$temporary/cargo-sources.json"
# The pinned generator omits the final newline expected by repository checks.
printf '\n' >> "$temporary/cargo-sources.json"
mv "$temporary/cargo-sources.json" "$root/package/flatpak/cargo-sources.json"
