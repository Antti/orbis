#!/usr/bin/env bash
set -euo pipefail

version=${1:?Usage: package-macos.sh VERSION TARGET BINARY OUTPUT_DIRECTORY}
target=${2:?missing target}
binary=${3:?missing binary}
output=${4:?missing output directory}
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]
case "$target" in
  x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "Unsupported macOS target: $target" >&2; exit 1 ;;
esac
mkdir -p "$output"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/orbis-package.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
name="orbis-pkg-util-$version-$target"
mkdir "$work_dir/$name"
install -m755 "$binary" "$work_dir/$name/orbis-pkg-util"
cp LICENSE-MIT LICENSE-APACHE "$work_dir/$name/"
if [[ ${MACOS_NOTARIZED:-false} == true ]]; then
  codesign --verify --strict "$work_dir/$name/orbis-pkg-util"
  printf '%s\n' 'Developer ID signed; Apple notarization accepted. CLI tickets require an online Gatekeeper check.' > "$work_dir/$name/BUILD-INFO.txt"
else
  printf '%s\n' 'Unsigned build; not notarized.' > "$work_dir/$name/BUILD-INFO.txt"
fi
"$work_dir/$name/orbis-pkg-util" --help
COPYFILE_DISABLE=1 tar -czf "$output/$name.tar.gz" -C "$work_dir" "$name"
# Verify executable permissions and contents survive the archive.
mkdir "$work_dir/unpacked"
tar -xzf "$output/$name.tar.gz" -C "$work_dir/unpacked"
"$work_dir/unpacked/$name/orbis-pkg-util" --help
