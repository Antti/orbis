# Package and filesystem validation

`compatibility.yml` runs actual disposable exFAT images on macOS and Linux,
APFS on macOS, and ext4 on Linux. Existing Windows tests run on the runner's NTFS
volume. The tests cover extraction, merged export, no-overwrite behavior,
manifest verification, and file-level resume. Images are detached and removed on
exit. For a local macOS run (requires disk-image permissions):

```sh
bash scripts/validation/filesystems.sh exfat
```

Public-package CI downloads [PS4Load v0.5.0](https://github.com/bucanero/ps4load/releases/tag/v0.5.0)
from its author, verifies a pinned package checksum, checks all available stored
integrity values, and compares 20 extracted files to checked-in hashes. It expects
exactly the known `sce_sys/license.info` omission (unavailable key slot 2), and
checks resume on Linux, macOS, and Windows. It does not run the extracted software.
The PKG payload is not committed or uploaded as an artifact. To reuse a local copy:

```sh
python3 scripts/validation/public_package.py target/debug/orbis-pkg-util --package /path/ps4load.pkg
```

For a matching local base/update pair, use a new directory with enough free space:

```sh
python3 scripts/validation/package_pair.py target/debug/orbis-pkg-util \
  /path/base.pkg /path/update.pkg /path/new-validation-directory
```

This verifies package integrity, extracts the base, plans and installs the update,
checks status, exports and verifies the merged directory, independently compares
the overlay hashes, rolls back, and rechecks the exported copy. Inputs remain
unchanged. Optional `--passcode-file` accepts a known package passcode. Incomplete
or failed checks stop immediately and remain recorded in `validation.json`;
this harness never labels an incomplete package as validated. Output is retained
for inspection. Only synthetic matching pairs run in public CI: no matching real
commercial pair is currently supplied, and no game launch is tested.
