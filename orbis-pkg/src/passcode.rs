//! Known-passcode key derivation. This does not bypass retail filesystem encryption.
//! Format reference: <https://github.com/maxton/LibOrbisPkg/blob/master/LibOrbisPkg/Util/Crypto.cs>
use crate::{Pkg, entry::EntryId};
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug)]
pub struct PasscodeError;
impl fmt::Display for PasscodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("passcode must be 32 printable ASCII characters and match the package entry-key digests")
    }
}
impl std::error::Error for PasscodeError {}

impl<R: AsRef<[u8]>> Pkg<R> {
    /// Derives all seven entry keys from a known 32-character package passcode.
    /// Checks every key digest before changing state. A wrong passcode is an
    /// error, even when a built-in RSA key could recover slot 3.
    pub fn load_passcode(&mut self, passcode: &[u8]) -> Result<(), PasscodeError> {
        if passcode.len() != 32 || !passcode.iter().all(|b| (0x20..=0x7e).contains(b)) {
            return Err(PasscodeError);
        }
        let (entry, _) = self
            .find_entry(EntryId::EntryKeys)
            .map_err(|_| PasscodeError)?;
        if entry.data_size() != 2048 || entry.is_encrypted() {
            return Err(PasscodeError);
        }
        let data = self.entry_data(&entry).map_err(|_| PasscodeError)?;
        let mut content_id = [0; 48];
        content_id[..36].copy_from_slice(&self.raw.as_ref()[0x40..0x64]);
        let id_hash = Sha256::digest(content_id);
        if data[..32] != id_hash[..] {
            return Err(PasscodeError);
        }
        let mut keys = [[0; 32]; 7];
        let mut difference = 0u8;
        for (i, key) in keys.iter_mut().enumerate() {
            let mut hash = Sha256::new();
            hash.update(Sha256::digest((i as u32).to_be_bytes()));
            hash.update(id_hash);
            hash.update(passcode);
            *key = hash.finalize().into();
            let digest = Sha256::digest(*key);
            for j in 0..32 {
                difference |= (digest[j] ^ key[j]) ^ data[32 + i * 32 + j];
            }
        }
        if difference != 0 {
            return Err(PasscodeError);
        }
        self.passcode_keys = Some(keys);
        // A previously loaded image key must be reloaded with the new entry keys.
        self.ekpfs.clear();
        Ok(())
    }
}
