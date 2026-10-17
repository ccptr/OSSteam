// SPDX-License-Identifier: GPL-2.0-or-later
//! Depot keys and the symmetric scheme they are used with (`content.md`
//! §2, §4, §5).

use std::fmt;

use aes::Aes256;
use aes::cipher::{
    BlockDecrypt, BlockDecryptMut, KeyInit, KeyIvInit, block_padding::Pkcs7,
    generic_array::GenericArray,
};
use zeroize::Zeroizing;

/// A depot's AES-256 key. Zeroed on drop; `Debug` hides it (D-10).
#[derive(Clone, PartialEq, Eq)]
pub struct DepotKey(Zeroizing<[u8; 32]>);

impl DepotKey {
    /// `None` unless `bytes` is exactly 32 bytes long.
    pub fn new(bytes: &[u8]) -> Option<Self> {
        let key: [u8; 32] = bytes.try_into().ok()?;
        Some(Self(Zeroizing::new(key)))
    }

    /// The key itself, for test fixtures that must encrypt with it.
    #[cfg(any(test, feature = "testing"))]
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for DepotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DepotKey(..)")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecryptError {
    #[error(
        "the ciphertext is {0} bytes; it must be a whole number of 16-byte blocks after a 16-byte IV"
    )]
    Length(usize),
    #[error("the padding is wrong; the key is probably not this depot's")]
    Padding,
}

/// Decrypts Steam's symmetric format: a 16-byte IV encrypted on its own
/// with AES-256-ECB, then AES-256-CBC with PKCS#7 padding under that IV.
pub fn decrypt(key: &DepotKey, data: &[u8]) -> Result<Vec<u8>, DecryptError> {
    if data.len() < 32 || !data.len().is_multiple_of(16) {
        return Err(DecryptError::Length(data.len()));
    }
    let (iv_enc, body) = data.split_at(16);
    let mut iv = GenericArray::clone_from_slice(iv_enc);
    Aes256::new(GenericArray::from_slice(&key.0[..])).decrypt_block(&mut iv);
    cbc::Decryptor::<Aes256>::new(GenericArray::from_slice(&key.0[..]), &iv)
        .decrypt_padded_vec_mut::<Pkcs7>(body)
        .map_err(|_| DecryptError::Padding)
}

/// The inverse of [`decrypt`], for test fixtures and the fake CDN. `iv` is
/// the plaintext IV.
#[cfg(any(test, feature = "testing"))]
pub fn encrypt(key: &DepotKey, iv: [u8; 16], data: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockEncrypt, BlockEncryptMut};
    let mut iv_enc = GenericArray::from(iv);
    Aes256::new(GenericArray::from_slice(&key.0[..])).encrypt_block(&mut iv_enc);
    let body = cbc::Encryptor::<Aes256>::new(GenericArray::from_slice(&key.0[..]), &iv.into())
        .encrypt_padded_vec_mut::<Pkcs7>(data);
    let mut out = iv_enc.to_vec();
    out.extend(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_the_wrong_key() {
        let key = DepotKey::new(&[7; 32]).unwrap();
        let ct = encrypt(&key, [3; 16], b"hello, depot");
        assert_eq!(ct.len(), 32);
        assert_eq!(decrypt(&key, &ct).unwrap(), b"hello, depot");

        let other = DepotKey::new(&[8; 32]).unwrap();
        // A wrong key yields garbage whose padding almost never checks out.
        assert_ne!(
            decrypt(&other, &ct).ok().as_deref(),
            Some(&b"hello, depot"[..])
        );
    }

    #[test]
    fn short_and_ragged_input_is_refused() {
        let key = DepotKey::new(&[7; 32]).unwrap();
        assert_eq!(decrypt(&key, &[0; 16]), Err(DecryptError::Length(16)));
        assert_eq!(decrypt(&key, &[0; 33]), Err(DecryptError::Length(33)));
    }

    #[test]
    fn keys_must_be_32_bytes_and_do_not_print() {
        assert!(DepotKey::new(&[0; 31]).is_none());
        let key = DepotKey::new(&[0xAB; 32]).unwrap();
        assert_eq!(format!("{key:?}"), "DepotKey(..)");
    }
}
