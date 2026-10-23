# Changelog

## [0.4.1] - 2026-10-23

### Fixed

- Correct false SHA-256 mismatches for encrypted PKG entries whose lengths are
  not multiples of 16 bytes. Verification now includes ciphertext padding and
  rejects truncated padding, while leaving unencrypted entry lengths unchanged.
