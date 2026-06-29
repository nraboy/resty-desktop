use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;

const ARGON2_MEM_KIB: u32 = 65536;
const ARGON2_ITER: u32 = 3;
const ARGON2_PARA: u32 = 4;

pub fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    let params =
        Params::new(ARGON2_MEM_KIB, ARGON2_ITER, ARGON2_PARA, Some(32)).map_err(|e| e.to_string())?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| e.to_string())?;
    Ok(key)
}

pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|e| e.to_string())?;
    Ok((nonce_bytes.to_vec(), ciphertext))
}

pub fn decrypt(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed — incorrect master password".to_string())
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &[u8] = b"test_salt_16byte";

    #[test]
    fn derive_key_is_deterministic() {
        let k1 = derive_key("password", SALT).unwrap();
        let k2 = derive_key("password", SALT).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_key_differs_on_different_passwords() {
        let k1 = derive_key("password", SALT).unwrap();
        let k2 = derive_key("other", SALT).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn derive_key_differs_on_different_salts() {
        let k1 = derive_key("password", b"salt_aaaaaaaaaa1").unwrap();
        let k2 = derive_key("password", b"salt_aaaaaaaaaa2").unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = derive_key("password", SALT).unwrap();
        let plaintext = b"hello, world";
        let (nonce, ciphertext) = encrypt(&key, plaintext).unwrap();
        let recovered = decrypt(&key, &nonce, &ciphertext).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn decrypt_fails_with_wrong_key() {
        let key = derive_key("password", SALT).unwrap();
        let wrong_key = derive_key("wrong", SALT).unwrap();
        let (nonce, ciphertext) = encrypt(&key, b"secret").unwrap();
        assert!(decrypt(&wrong_key, &nonce, &ciphertext).is_err());
    }

    #[test]
    fn decrypt_fails_with_tampered_ciphertext() {
        let key = derive_key("password", SALT).unwrap();
        let (nonce, mut ciphertext) = encrypt(&key, b"secret").unwrap();
        ciphertext[0] ^= 0xFF;
        assert!(decrypt(&key, &nonce, &ciphertext).is_err());
    }

    #[test]
    fn encrypt_produces_different_nonces_each_call() {
        let key = derive_key("password", SALT).unwrap();
        let (nonce1, _) = encrypt(&key, b"same").unwrap();
        let (nonce2, _) = encrypt(&key, b"same").unwrap();
        // Nonces are random; same in the same call only by extreme luck.
        // This will virtually never collide but is probabilistic.
        assert_ne!(nonce1, nonce2);
    }

}
