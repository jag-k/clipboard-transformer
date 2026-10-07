#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY REPOSITORY_URL BASE64_GPG_KEY" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
output_directory="$1"
repository_url="${2%/}/"
gpg_key="$3"

if [[ "${repository_url}" != https://* ]]; then
  echo "repository URL must use HTTPS" >&2
  exit 2
fi
if [[ -z "${gpg_key}" ]]; then
  echo "base64 GPG key must not be empty" >&2
  exit 2
fi

mkdir -p "${output_directory}/icons"
# Software centers show this icon while installing from the .flatpakref,
# before the app's own AppStream data is available.
install -m0644 "${root}/assets/generated/linux/app-icon.png" \
  "${output_directory}/icons/dev.jag_k.clipboard_transformer.png"

printf '%s\n' \
  '[Flatpak Repo]' \
  'Title=jag-k Flatpak Repository' \
  "Url=${repository_url}repo/" \
  'Homepage=https://github.com/jag-k/flatpak-repo' \
  'Comment=Flatpak applications published by jag-k' \
  'Description=Flatpak applications published and signed by jag-k' \
  "GPGKey=${gpg_key}" \
  > "${output_directory}/jag-k.flatpakrepo"

printf '%s\n' \
  '[Flatpak Ref]' \
  'Name=dev.jag_k.clipboard_transformer' \
  'Branch=stable' \
  'Title=Clipboard Transformer' \
  'Comment=Transform clipboard content with configurable rules' \
  'Homepage=https://github.com/jag-k/clipboard-transformer' \
  "Icon=${repository_url}icons/dev.jag_k.clipboard_transformer.png" \
  "Url=${repository_url}repo/" \
  'RuntimeRepo=https://flathub.org/repo/flathub.flatpakrepo' \
  'IsRuntime=false' \
  'SuggestRemoteName=jag-k' \
  "GPGKey=${gpg_key}" \
  > "${output_directory}/clipboard-transformer.flatpakref"

touch "${output_directory}/.nojekyll"
