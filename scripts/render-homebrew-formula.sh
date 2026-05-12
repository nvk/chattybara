#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  printf 'usage: %s <version> <url> <sha256> <output>\n' "$0" >&2
  exit 2
fi

version="${1#v}"
url="$2"
sha256="$3"
output="$4"
template="packaging/homebrew/chattybara.rb.in"

mkdir -p "$(dirname "$output")"
sed \
  -e "s|@VERSION@|${version}|g" \
  -e "s|@URL@|${url}|g" \
  -e "s|@SHA256@|${sha256}|g" \
  "$template" > "$output"

printf 'formula=%s\n' "$output"
