#!/usr/bin/env bash
# Exercise real disposable filesystem images; never format an existing device.
set -euo pipefail
filesystem=${1:-exfat}
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/orbis-filesystem.XXXXXX")
mount_dir="$work_dir/mount"
mkdir "$mount_dir"
mounted=false
cleanup() {
  if [[ $mounted == true ]]; then
    if [[ $(uname -s) == Darwin ]]; then
      hdiutil detach "$mount_dir" || return
    else
      sudo umount "$mount_dir" || return
    fi
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT
if [[ $(uname -s) == Darwin ]]; then
  case "$filesystem" in exfat) format=ExFAT ;; apfs) format=APFS ;; *) exit 2 ;; esac
  hdiutil create -size 256m -fs "$format" -volname ORBISCI "$work_dir/volume.dmg"
  hdiutil attach "$work_dir/volume.dmg" -mountpoint "$mount_dir" -nobrowse
else
  truncate -s 256M "$work_dir/volume.img"
  case "$filesystem" in
    exfat)
      mkfs.exfat "$work_dir/volume.img"
      sudo mount -o "loop,uid=$(id -u),gid=$(id -g)" "$work_dir/volume.img" "$mount_dir"
      ;;
    ext4)
      mkfs.ext4 -F "$work_dir/volume.img"
      sudo mount -o loop "$work_dir/volume.img" "$mount_dir"
      sudo chown "$(id -u):$(id -g)" "$mount_dir"
      ;;
    *) exit 2 ;;
  esac
fi
mounted=true
export ORBIS_TEST_EXTRACT_ROOT="$mount_dir" ORBIS_TEST_EXPORT_ROOT="$mount_dir"
export ORBIS_TEST_FILESYSTEM="$filesystem"
cargo test -p orbis-pkg-util --lib --locked configured_external_filesystem -- --nocapture
