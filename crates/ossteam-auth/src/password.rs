// SPDX-License-Identifier: GPL-2.0-or-later
//! Password encryption for credential sign-in (`auth.md` §2 steps 1, 2).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ossteam_types::Secret;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPublicKey};

use crate::AuthError;

/// Encrypts `password` with RSA PKCS#1 v1.5 under the key the server gave
/// (hex modulus and exponent) and returns it base64-encoded. The password
/// itself is dropped, and so zeroed, when this returns.
pub fn encrypt(
    modulus_hex: &str,
    exponent_hex: &str,
    password: Secret,
) -> Result<String, AuthError> {
    let n = BigUint::parse_bytes(modulus_hex.as_bytes(), 16)
        .ok_or_else(|| AuthError::BadKey("modulus is not hex".into()))?;
    let e = BigUint::parse_bytes(exponent_hex.as_bytes(), 16)
        .ok_or_else(|| AuthError::BadKey("exponent is not hex".into()))?;
    let key = RsaPublicKey::new(n, e).map_err(|err| AuthError::BadKey(err.to_string()))?;
    let mut plaintext = zeroize::Zeroizing::new(password.expose().as_bytes().to_vec());
    drop(password);
    let ciphertext = key
        .encrypt(&mut rand_core::OsRng, Pkcs1v15Encrypt, &plaintext)
        .map_err(|err| AuthError::Encrypt(err.to_string()))?;
    zeroize::Zeroize::zeroize(&mut *plaintext);
    Ok(STANDARD.encode(ciphertext))
}

#[cfg(test)]
mod tests {
    use rsa::RsaPrivateKey;
    use rsa::traits::PublicKeyParts;

    use super::*;

    #[test]
    fn round_trips_through_the_private_key() {
        let private = RsaPrivateKey::new(&mut rand_core::OsRng, 1024).unwrap();
        let public = private.to_public_key();
        let enc = encrypt(
            &public.n().to_str_radix(16),
            &public.e().to_str_radix(16),
            Secret::new("correct horse".into()),
        )
        .unwrap();
        let raw = STANDARD.decode(enc).unwrap();
        assert_eq!(raw.len(), 128);
        assert_eq!(
            private.decrypt(Pkcs1v15Encrypt, &raw).unwrap(),
            b"correct horse"
        );
    }

    #[test]
    fn rejects_garbage_keys() {
        assert!(matches!(
            encrypt("zz", "010001", Secret::new("x".into())),
            Err(AuthError::BadKey(_))
        ));
    }
}
