use std::fmt;
use zeroize::Zeroizing;

/// A session-only credential. Debug output never includes its contents.
pub struct Passcode(Zeroizing<String>);

impl Passcode {
    pub fn new(value: Zeroizing<String>) -> Result<Self, &'static str> {
        if value.len() != 32 || !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err("Enter exactly 32 printable ASCII characters.");
        }
        Ok(Self(value))
    }

    pub(super) fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for Passcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passcode([redacted])")
    }
}
