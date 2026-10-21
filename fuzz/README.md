# Parser fuzzing

These targets call the production parsers with arbitrary bytes under AddressSanitizer,
with debug assertions and integer overflow checks enabled. They never extract files
or connect to the network.

| Target | Coverage |
| --- | --- |
| `pkg` | Header/entry ranges, known passcode validation, stored SHA-256 checks, name tables, safe relative paths, entry data and embedded SFO |
| `sfo` | Typed SFO fields, offset/count validation and application metadata |
| `pfs` | Slice and positional readers, encrypted/signed images, S32 data and S64 inode-table signatures, bounded directory traversal and file reads |
| `pfsc` | Compressed/stored/sparse block tables, decompression, partial blocks, EOF and extreme read offsets |

## Run locally

Install a nightly toolchain and cargo-fuzz, then run from the repository root:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --version 0.13.2 --locked
cargo fetch --manifest-path fuzz/Cargo.toml --locked
python3 fuzz/generate_corpus.py
CARGO_NET_OFFLINE=true cargo +nightly fuzz run pkg -- \
  -max_total_time=60 -max_len=1048576 -timeout=10 -rss_limit_mb=1024
```

Repeat for `sfo`, `pfs` and `pfsc`. See the upstream
[cargo-fuzz guide](https://rust-fuzz.github.io/book/cargo-fuzz/guide.html).
The isolated fuzz workspace keeps nightly and libFuzzer out of normal builds;
its lockfile is committed and shares the production dependency versions. CI fetches with `--locked`, then builds offline.

`generate_corpus.py` writes deterministic synthetic seeds independently of the Rust
parsers: valid typed SFO, safe/unsafe names, signed direct/indirect PFS trees,
compressed/stored/sparse PFSC, and truncated input. It only overwrites its named
seeds; locally discovered inputs remain. No real package contents or private keys are committed.

Inputs are capped at 1 MiB. After parsing, harnesses limit PKG entry reads to 1024,
PFS traversal to 128 entries, and integrity work to 512 cancellation polls. This
keeps each mutation useful; it does not establish behavior for larger files or
unbounded traversals. PFSC readers probe a fixed set of offsets without allocating
the declared decoded image size. Parser errors are expected; panics, sanitizer
findings, unsafe output paths, timeouts and memory-limit breaches are failures.

## CI and reproduction

Pull requests, master pushes and manual runs fuzz each target for 60 seconds.
The Monday scheduled run uses 15 minutes per target. Each target has a 10-second
input timeout, 1 GiB resident-memory limit and a 30-minute job timeout. Failed
runs upload reproducing inputs for 14 days. The CI corpus starts from the
synthetic seeds on each run; longer campaigns can retain a local corpus.

Reproduce and minimize a saved failure:

```sh
cargo +nightly fuzz run pfs fuzz/artifacts/pfs/crash-REPLACE_WITH_HASH
cargo +nightly fuzz tmin pfs fuzz/artifacts/pfs/crash-REPLACE_WITH_HASH
```

Turn every confirmed bug into a small regression test before fixing it. Keep
real packages out of commits and CI artifacts; only use distribution-safe
synthetic or minimized inputs. Fuzzing supplements the regression suite and
[real-package compatibility checks](../TESTING.md); a bounded run is not a proof
that all inputs are safe.
