#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
  echo "usage: $0 OUTPUT_DIRECTORY REPOSITORY_URL BASE64_GPG_KEY" >&2
  exit 2
fi

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

mkdir -p "${output_directory}"

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
  "Url=${repository_url}repo/" \
  'RuntimeRepo=https://flathub.org/repo/flathub.flatpakrepo' \
  'IsRuntime=false' \
  'SuggestRemoteName=jag-k' \
  "GPGKey=${gpg_key}" \
  > "${output_directory}/clipboard-transformer.flatpakref"

touch "${output_directory}/.nojekyll"
