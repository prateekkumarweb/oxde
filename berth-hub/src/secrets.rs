use std::path::Path;

use anyhow::Context;
use base64::Engine;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};

pub struct SecretsKey(Zeroizing<[u8; 32]>);

impl SecretsKey {
    pub fn encrypt(&self, plaintext: &str) -> AppResult<String> {
        let cipher = ChaCha20Poly1305::new((&*self.0).into());
        let nonce_bytes = rand::random::<[u8; 12]>();
        let nonce: Nonce = nonce_bytes.into();
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|err| {
                tracing::error!(?err, "failed to encrypt a secret");
                AppError::CorruptData("secret value could not be encrypted".to_string())
            })?;
        let mut out = nonce_bytes.to_vec();
        out.extend_from_slice(&ciphertext);
        Ok(base64::engine::general_purpose::STANDARD.encode(out))
    }

    pub fn decrypt(&self, stored: &str) -> AppResult<String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(stored)
            .map_err(|err| {
                tracing::error!(?err, "failed to base64-decode a stored secret");
                AppError::CorruptData("secret value could not be decoded".to_string())
            })?;
        if raw.len() < 12 {
            tracing::error!("stored secret is shorter than one nonce");
            return Err(AppError::CorruptData(
                "secret value could not be decoded".to_string(),
            ));
        }
        let (nonce, ciphertext) = raw.split_at(12);
        let nonce = Nonce::try_from(nonce).map_err(|err| {
            tracing::error!(?err, "stored secret nonce had the wrong length");
            AppError::CorruptData("secret value could not be decoded".to_string())
        })?;
        let cipher = ChaCha20Poly1305::new((&*self.0).into());
        let plaintext = cipher.decrypt(&nonce, ciphertext).map_err(|err| {
            tracing::error!(?err, "failed to decrypt a stored secret");
            AppError::CorruptData("secret value could not be decrypted".to_string())
        })?;
        String::from_utf8(plaintext).map_err(|err| {
            tracing::error!(?err, "decrypted secret was not valid utf-8");
            AppError::CorruptData("secret value could not be decoded".to_string())
        })
    }
}

pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<SecretsKey> {
    let path = data_dir.join("secrets.key");

    match std::fs::read(&path) {
        Ok(bytes) => {
            let key: [u8; 32] = bytes.try_into().map_err(|_| {
                anyhow::anyhow!(
                    "secrets key file at {} has the wrong length",
                    path.display()
                )
            })?;
            return Ok(SecretsKey(Zeroizing::new(key)));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read secrets key file at {}", path.display()));
        }
    }

    let key = rand::random::<[u8; 32]>();
    std::fs::write(&path, key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(SecretsKey(Zeroizing::new(key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let key = SecretsKey(Zeroizing::new([7u8; 32]));
        let ciphertext = key.encrypt("hunter2").unwrap();
        assert_ne!(ciphertext, "hunter2");
        assert_eq!(key.decrypt(&ciphertext).unwrap(), "hunter2");
    }

    #[test]
    fn detects_tampering() {
        let key = SecretsKey(Zeroizing::new([7u8; 32]));
        let mut ciphertext = base64::engine::general_purpose::STANDARD
            .decode(key.encrypt("hunter2").unwrap())
            .unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 1;
        let tampered = base64::engine::general_purpose::STANDARD.encode(ciphertext);
        assert!(key.decrypt(&tampered).is_err());
    }

    #[test]
    fn load_or_generate_persists_across_calls() {
        let dir =
            std::env::temp_dir().join(format!("berth-secrets-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let key1 = load_or_generate(&dir).unwrap();
        let key2 = load_or_generate(&dir).unwrap();
        let ciphertext = key1.encrypt("hunter2").unwrap();
        assert_eq!(key2.decrypt(&ciphertext).unwrap(), "hunter2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_or_generate_fails_rather_than_replacing_an_unreadable_key() {
        let dir =
            std::env::temp_dir().join(format!("berth-secrets-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(dir.join("secrets.key")).unwrap();
        assert!(load_or_generate(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
