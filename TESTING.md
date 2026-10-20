# Testing

Run the same checks used by GitHub Actions:

```sh
cargo test --workspace --all-features --locked
cargo test -p orbis-pkg-util --no-default-features --locked
cargo test --workspace --all-features --release --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

CI runs the debug tests on Linux, macOS, and Windows, and release tests on Linux.
Release tests matter because integer overflow behaves differently between debug
and optimized builds.

The tests construct small synthetic PKG, PFS, and PFSC images. They cover malformed
headers, block reads, decompression, extraction boundaries, and overwrite policy
without downloading game data. Extraction tests write only into temporary
directories. Unix-specific symlink tests run on Linux and macOS.

Library README examples are included in doctests. CLI integration tests run
synthetic packages through metadata inspection, filesystem listing, JSON output,
selected extraction, verification, completion generation, and failure handling.
They check exit codes and stdout/stderr separately, including warnings in quiet
mode and closed-pipe handling on Unix. Library tests cover atomic replacement,
concurrent destination creation, cancellation, and cleanup after corrupt input.
The shared fixture builders live in `orbis-pkg-util/tests/common/`.

Installer tests encode SFO identity/version metadata independently of the parser
and build synthetic base/update pairs. They cover read-only planning, delta and
identity rejection, complete update replacement, one-level rollback, modified or
unmanaged directories, concurrent operation locks, cancellation, corrupt payloads,
and recovery after simulated process exits at each persisted activation/rollback
boundary. Recovery is repeated to check idempotency. CLI tests verify dry-run JSON,
quiet installation, rollback, and actionable recovery errors. These tests run
through the existing Linux, macOS, and Windows jobs; library-only builds include
the installer as well.

Synthetic fixtures do not replace compatibility checks against real packages.
Before a release, extract a known package and compare file contents with a trusted
reference extraction. For installation, use a representative original base plus
full-file update, compare the resulting update tree with a trusted extraction,
check the version in shadPS4, and exercise upgrade/rollback while the emulator is
stopped. No real base/update pair was available during implementation.
