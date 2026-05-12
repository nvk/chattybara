#!/usr/bin/env bash
set -euo pipefail

version="${1:-}"
if [[ -z "$version" ]]; then
  version="$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)"
fi
version="${version#v}"

target="${TARGET:-$(rustc -vV | awk '/^host:/ { print $2 }')}"
name="chattybara-${version}-${target}"
archive="dist/${name}.tar.gz"
checksum="${archive}.sha256"
package_dir="dist/${name}"

workspace_root="$(pwd -P)"
unit_sep=$'\x1f'
release_rustflags=()
if [[ -n "${HOME:-}" ]]; then
  release_rustflags+=("--remap-path-prefix=${HOME}=/home/builder")
fi
release_rustflags+=("--remap-path-prefix=${workspace_root}=.")

encoded_release_rustflags=""
for flag in "${release_rustflags[@]}"; do
  if [[ -n "$encoded_release_rustflags" ]]; then
    encoded_release_rustflags+="$unit_sep"
  fi
  encoded_release_rustflags+="$flag"
done

if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}${unit_sep}${encoded_release_rustflags}"
else
  export CARGO_ENCODED_RUSTFLAGS="$encoded_release_rustflags"
fi

if [[ -n "${TARGET:-}" ]]; then
  cargo build --release -p chattybara-cli --locked --target "$target"
  binary="target/${target}/release/chattybara"
else
  cargo build --release -p chattybara-cli --locked
  binary="target/release/chattybara"
fi

rm -rf "$package_dir"
mkdir -p "$package_dir"
cp "$binary" "$package_dir/chattybara"
strip "$package_dir/chattybara" 2>/dev/null || true
cp LICENSE README.md "$package_dir/"

release_notes="docs/release-notes-v${version}.md"
if [[ -f "$release_notes" ]]; then
  cp "$release_notes" "$package_dir/RELEASE-NOTES.md"
fi

tar -C dist -czf "$archive" "$name"
shasum -a 256 "$archive" > "$checksum"

printf 'archive=%s\n' "$archive"
printf 'sha256=%s\n' "$(awk '{ print $1 }' "$checksum")"
