# Changelog

## [0.5.0] - 2026-10-25

### Added

- Desktop package verification with full and metadata-only scopes, cancellation,
  distinct incomplete/failure results, and saveable JSON summary reports.
- Session-only desktop passcodes, validated against all entry-key digests before
  use in inspection, extraction, installation, or verification.
- Desktop merged export with a version/space preview, resume with conflict
  protection, and automatic verification of the finished export manifest.
- Read-only library export previews for resumable destinations, validating
  existing files and estimating space for missing files.

### Changed

- Enable runtime-detected ARM AES and SHA-256 acceleration in macOS/Linux builds
  for faster package verification and extraction. CPUs without those instructions
  retain the software fallback; x86 crypto backends are unchanged.

## [0.4.1] - 2026-10-23

### Fixed

- Correct false SHA-256 mismatches for encrypted PKG entries whose lengths are
  not multiples of 16 bytes. Verification now includes ciphertext padding and
  rejects truncated padding, while leaving unencrypted entry lengths unchanged.
