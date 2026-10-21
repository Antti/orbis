# orbis-integrity

Shared check events for PKG SHA-256 and PFS HMAC verification. This crate has no
runtime dependencies. It keeps status/reason definitions consistent without
coupling the two binary-format parsers or requiring serialization in either one.

`Check` borrows its location for the duration of the observer call. Consumers
can stream records, aggregate them, or copy them for storage. `IntegrityReport`
retains only checked/unavailable counts. Matching stored hashes establishes
consistency, not publisher authenticity.
