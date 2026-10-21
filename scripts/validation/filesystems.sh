#!/usr/bin/env bash
# Exercise real disposable filesystem images; never format an existing device.
set -euo pipefail
filesystem=${1:-exfat}
expected_filesystem=$filesystem
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/orbis-filesystem.XXXXXX")
mount_dir="$work_dir/mount"
mkdir "$mount_dir"
mounted=false
cleanup() {
  if [[ $mounted == true ]]; then
    if [[ $(uname -s) == Darwin ]]; then
      # Spotlight/volume services can briefly keep a newly mounted APFS image
      # busy after the tests exit. Retry, then force only our disposable image.
      detached=false
      for attempt in 1 2 3; do
        if hdiutil detach "$mount_dir"; then detached=true; break; fi
        sleep "$attempt"
      done
      if [[ $detached == false ]]; then hdiutil detach -force "$mount_dir" || return; fi
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
    exfat|exfat-fuse)
      mkfs.exfat "$work_dir/volume.img"
      if [[ $filesystem == exfat ]] && sudo modprobe exfat && sudo mount -t exfat -o "loop,uid=$(id -u),gid=$(id -g)" "$work_dir/volume.img" "$mount_dir"; then
        echo 'Testing kernel exFAT'
      else
        echo 'Testing the real exFAT image through FUSE'
        sudo mount.exfat-fuse -o "allow_other,uid=$(id -u),gid=$(id -g)" "$work_dir/volume.img" "$mount_dir"
        # statfs identifies the generic FUSE driver rather than its exFAT subtype.
        expected_filesystem=unknown
      fi
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
touch "$mount_dir/.metadata_never_index"
export ORBIS_TEST_EXTRACT_ROOT="$mount_dir" ORBIS_TEST_EXPORT_ROOT="$mount_dir"
export ORBIS_TEST_FILESYSTEM="$expected_filesystem"
cargo test -p orbis-pkg-util --lib --locked configured_external_filesystem -- --nocapture
