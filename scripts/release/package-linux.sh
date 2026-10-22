#!/usr/bin/env bash
# Package one static musl binary as an AppImage and a Flatpak bundle.
set -euo pipefail

arch=${4:-x86_64}
case "$arch" in
  x86_64)
    tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    ;;
  aarch64)
    tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
    ;;
  *) echo "Unsupported Linux architecture: $arch" >&2; exit 1 ;;
esac

version=${1:?Usage: package-linux.sh VERSION BINARY OUTPUT_DIRECTORY [ARCH]}
binary=$(realpath "${2:?missing binary}")
output=$(realpath -m "${3:?missing output directory}")
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]
mkdir -p "$output"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/orbis-package.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
app_id=io.github.Antti.Orbis

# Pin both the build tool and the runtime embedded in the resulting AppImage.
curl --fail --location --retry 3 \
  "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage" \
  -o "$work_dir/appimagetool"
curl --fail --location --retry 3 \
  "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$arch" \
  -o "$work_dir/runtime"
printf '%s  %s\n' \
  "$tool_sha" "$work_dir/appimagetool" \
  "$runtime_sha" "$work_dir/runtime" \
  | sha256sum --check
chmod +x "$work_dir/appimagetool"

app_dir="$work_dir/Orbis.AppDir"
mkdir -p "$app_dir/usr/bin" "$app_dir/usr/share/licenses/orbis"
install -m755 "$binary" "$app_dir/usr/bin/orbis-pkg-util"
install -m755 packaging/AppRun "$app_dir/AppRun"
cp packaging/orbis.desktop "$app_dir/orbis.desktop"
cp "packaging/$app_id.svg" "$app_dir/$app_id.svg"
cp LICENSE-MIT LICENSE-APACHE "$app_dir/usr/share/licenses/orbis/"
ARCH="$arch" VERSION="$version" "$work_dir/appimagetool" --appimage-extract-and-run \
  --no-appstream --runtime-file "$work_dir/runtime" "$app_dir" \
  "$output/orbis-pkg-util-$version-linux-$arch.AppImage"
"$output/orbis-pkg-util-$version-linux-$arch.AppImage" --appimage-extract-and-run --help

# Rust is built outside Flatpak. The SDK is needed only to initialize its metadata.
runtime_version=25.08
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user --noninteractive --no-related flathub \
  "org.freedesktop.Platform/$arch/$runtime_version" \
  "org.freedesktop.Sdk/$arch/$runtime_version"
flatpak build-init --arch="$arch" "$work_dir/flatpak" "$app_id" \
  org.freedesktop.Sdk org.freedesktop.Platform "$runtime_version"
files="$work_dir/flatpak/files"
mkdir -p "$files/bin" "$files/share/metainfo" "$files/share/licenses/orbis"
install -m755 "$binary" "$files/bin/orbis-pkg-util"
cp LICENSE-MIT LICENSE-APACHE "$files/share/licenses/orbis/"
release_date=$(date -u +%Y-%m-%d)
sed -e "s/@VERSION@/$version/g" -e "s/@DATE@/$release_date/g" \
  "packaging/$app_id.metainfo.xml" > "$files/share/metainfo/$app_id.metainfo.xml"
flatpak build-finish --no-inherit-permissions --command=orbis-pkg-util "$work_dir/flatpak"
flatpak build-export "$work_dir/repo" "$work_dir/flatpak" stable
bundle="$output/orbis-pkg-util-$version-linux-$arch.flatpak"
flatpak build-bundle --arch="$arch" \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$work_dir/repo" "$bundle" "$app_id" stable
# Test the distributable itself, including its command and runtime metadata.
flatpak install --user --noninteractive --bundle "$bundle"
dbus-run-session -- flatpak run --user "$app_id" --help
