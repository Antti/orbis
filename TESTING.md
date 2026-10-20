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

Library README examples are included in doctests. CLI smoke tests check the
documented commands and help output.

Synthetic fixtures do not replace compatibility checks against real packages.
Before a release, extract a known package and compare file contents with a trusted
reference extraction.
