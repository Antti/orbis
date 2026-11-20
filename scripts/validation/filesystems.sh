#!/usr/bin/env bash
# Exercise real disposable filesystem images; never format an existing device.
set -euo pipefail
filesystem=${1:-exfat}
expected_filesystem=$filesystem
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/orbis-filesystem.XXXXXX")
mount_dir="$work_dir/mount"
mkdir "$mount_dir"
mounted=false
resume_mount="$work_dir/resume-volume"
resume_mounted=false
loop_device=''
cleanup() {
  if [[ $resume_mounted == true ]]; then sudo umount "$resume_mount" || return; fi
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
  if [[ -n $loop_device ]]; then sudo losetup --detach "$loop_device" || return; fi
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
        # libfuse3 mounts exFAT as fuseblk and requires a block device.
        loop_device=$(sudo losetup --find --show "$work_dir/volume.img")
        sudo mount.exfat-fuse -o "allow_other,uid=$(id -u),gid=$(id -g)" "$loop_device" "$mount_dir"
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
# An existing export can itself be a mount point. Bind an empty directory so
# the volume root's filesystem metadata is outside the exported game.
if [[ $(uname -s) == Linux ]]; then
  mkdir "$mount_dir/empty-export" "$resume_mount"
  sudo mount --bind "$mount_dir/empty-export" "$resume_mount"
  resume_mounted=true
  export ORBIS_TEST_RESUME_MOUNT="$resume_mount"
fi
cargo test -p orbis-cli --lib --locked configured_external_filesystem -- --nocapture
