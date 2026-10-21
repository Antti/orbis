//! Shared, allocation-free events for stored hash verification.
//!
//! Parsers stream events to a fallible observer instead of retaining a record
//! for every block. A passed check establishes consistency, not authenticity.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Passed,
    Unavailable,
    Failed,
    NotApplicable,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    MissingDigest,
    MissingKey,
    UnsignedImage,
    ContiguousExtent,
    UnsupportedLayout,
    DigestMismatch,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingDigest => "missing_digest",
            Self::MissingKey => "missing_key",
            Self::UnsignedImage => "unsigned_image",
            Self::ContiguousExtent => "contiguous_extent",
            Self::UnsupportedLayout => "unsupported_layout",
            Self::DigestMismatch => "digest_mismatch",
        }
    }
}

/// One check, or an explicitly counted group of unavailable checks.
#[derive(Clone, Copy, Debug)]
pub struct Check<'a> {
    /// Parser-generated location, e.g. `PKG header` or `inode 3 data block 25`.
    pub scope: &'a str,
    pub status: Status,
    pub reason: Option<Reason>,
    /// Zero for a not-applicable image; otherwise the number of checks covered.
    pub count: usize,
}

#[derive(Debug, Default)]
pub struct IntegrityReport {
    pub checked: usize,
    pub missing: usize,
}

impl IntegrityReport {
    /// Emit a result and update totals. Observer errors stop verification.
    pub fn record(
        &mut self,
        scope: &str,
        status: Status,
        reason: Option<Reason>,
        count: usize,
        observer: &mut impl FnMut(Check<'_>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        observer(Check {
            scope,
            status,
            reason,
            count,
        })?;
        match status {
            Status::Passed => self.checked += count,
            Status::Unavailable => self.missing += count,
            _ => {}
        }
        Ok(())
    }
}
