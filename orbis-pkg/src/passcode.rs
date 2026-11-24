//! Known-passcode key derivation. This does not bypass retail filesystem encryption.
//! Format reference: <https://github.com/maxton/LibOrbisPkg/blob/master/LibOrbisPkg/Util/Crypto.cs>
use crate::{EntryDataError, FindEntryError, Pkg, entry::EntryId};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use zerocopy::IntoBytes;

/// Number of entry-key slots derivable from a known package passcode.
pub const ENTRY_KEY_COUNT: usize = 7;

#[derive(Debug, Snafu)]
#[snafu(display(
    "passcode must be 32 printable ASCII characters and match the package entry-key digests"
))]
pub struct PasscodeError;

#[derive(Debug, Snafu)]
#[snafu(module)]
#[non_exhaustive]
pub enum PasscodeValidationError {
    #[snafu(transparent)]
    InvalidPasscode { source: PasscodeError },
    #[snafu(display("missing entry-key record"))]
    MissingRecord,
    #[snafu(display("cannot read the entry-key record"))]
    ReadEntry { source: FindEntryError },
    #[snafu(display("encrypted entry-key record cannot be used for passcode validation"))]
    EncryptedRecord,
    #[snafu(display("invalid entry-key record size (expected 2048 bytes)"))]
    InvalidSize,
    #[snafu(display("cannot read entry-key record data"))]
    ReadData { source: EntryDataError },
    #[snafu(display("entry-key record does not match the package content ID"))]
    ContentIdMismatch,
}

impl<R: AsRef<[u8]>> Pkg<R> {
    /// Checks the metadata needed to validate a passcode without asking for one.
    pub fn check_passcode_metadata(&self) -> Result<(), PasscodeValidationError> {
        self.passcode_record().map(|_| ())
    }

    fn passcode_record(&self) -> Result<Vec<u8>, PasscodeValidationError> {
        let (entry, _) = self
            .find_entry(EntryId::EntryKeys)
            .map_err(|source| match source {
                FindEntryError::NotFound => PasscodeValidationError::MissingRecord,
                source => PasscodeValidationError::ReadEntry { source },
            })?;
        if entry.is_encrypted() {
            return Err(PasscodeValidationError::EncryptedRecord);
        }
        if entry.data_size() != 2048 {
            return Err(PasscodeValidationError::InvalidSize);
        }
        let data = self
            .entry_data(&entry)
            .map_err(|source| PasscodeValidationError::ReadData { source })?;
        let mut content_id = [0; 48];
        content_id[..36].copy_from_slice(self.header().content_id().as_bytes());
        if data[..32] != Sha256::digest(content_id)[..] {
            return Err(PasscodeValidationError::ContentIdMismatch);
        }
        Ok(data)
    }

    /// Derives all seven entry keys from a known 32-character package passcode.
    /// Checks every key digest before changing state. A wrong passcode is an
    /// error, even when a built-in RSA key could recover slot 3.
    pub fn load_passcode(&mut self, passcode: &[u8]) -> Result<(), PasscodeError> {
        self.load_passcode_with_diagnostics(passcode)
            .map_err(|_| PasscodeError)
    }

    /// Loads the keys and distinguishes unusable package metadata from a rejected passcode.
    pub fn load_passcode_with_diagnostics(
        &mut self,
        passcode: &[u8],
    ) -> Result<(), PasscodeValidationError> {
        if passcode.len() != 32 || !passcode.iter().all(|b| (0x20..=0x7e).contains(b)) {
            return Err(PasscodeError.into());
        }
        let data = self.passcode_record()?;
        let mut keys = [[0; 32]; ENTRY_KEY_COUNT];
        let mut difference = 0u8;
        for (i, key) in keys.iter_mut().enumerate() {
            let mut hash = Sha256::new();
            hash.update(Sha256::digest((i as u32).to_be_bytes()));
            hash.update(&data[..32]);
            hash.update(passcode);
            *key = hash.finalize().into();
            let digest = Sha256::digest(*key);
            for j in 0..32 {
                difference |= (digest[j] ^ key[j]) ^ data[32 + i * 32 + j];
            }
        }
        if difference != 0 {
            return Err(PasscodeError.into());
        }
        self.passcode_keys = Some(keys);
        // A previously loaded image key must be reloaded with the new entry keys.
        self.ekpfs.clear();
        Ok(())
    }
}
