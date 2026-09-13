//! Signing key material, ported from `IKeyMaterialService` and
//! `DevKeyMaterialService`.
//!
//! The development provider generates an RSA-2048 key on first use and
//! persists it as `{"Kid": …, "PrivateKeyPkcs8Base64": …}` — the same file the
//! C# provider writes, so an existing dev key loads unchanged.
//!
//! Unlike the original, the file is **not** committed. The C# repository has a
//! real RSA private key in git; this one is generated on first run and
//! gitignored (F6 in the plan).

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::{DecodingKey, EncodingKey};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use shared_kernel::REDACTED;

/// Modulus size the original generates.
pub const KEY_SIZE_BITS: usize = 2048;

/// The persisted form of a development key.
///
/// Member names are PascalCase because that is what the C# provider writes.
///
/// No derived `Debug`: the second member is the signing key itself, and a
/// process that logged one could have every token it ever issued forged.
#[derive(Clone, Serialize, Deserialize)]
struct PersistedKey {
    #[serde(rename = "Kid")]
    kid: String,
    #[serde(rename = "PrivateKeyPkcs8Base64")]
    private_key_pkcs8_base64: String,
}

impl std::fmt::Debug for PersistedKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersistedKey")
            .field("kid", &self.kid)
            .field("private_key_pkcs8_base64", &REDACTED)
            .finish()
    }
}

/// Something went wrong producing or loading key material.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The key could not be generated.
    #[error("failed to generate an RSA signing key")]
    Generate(#[source] rsa::Error),

    /// The key could not be encoded or decoded.
    #[error("the signing key could not be encoded: {0}")]
    Encoding(String),

    /// The key file could not be read or written.
    #[error("failed to access the signing key at {path}")]
    File {
        /// The file involved.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The provider is not usable in this environment.
    #[error("{0}")]
    Unavailable(String),
}

/// Signing and validation material, port of `IKeyMaterialService`.
pub trait KeyMaterial: Send + Sync {
    /// The key tokens are signed with.
    fn signing_key(&self) -> &EncodingKey;

    /// The keys tokens are validated against.
    fn validation_key(&self) -> &DecodingKey;

    /// The `kid` header value.
    fn current_key_id(&self) -> &str;

    /// The JWKS document `/.well-known/jwks.json` serves.
    fn jwks(&self) -> serde_json::Value;
}

/// The development provider.
pub struct DevKeyMaterial {
    kid: String,
    encoding: EncodingKey,
    decoding: DecodingKey,
    modulus: String,
    exponent: String,
    /// Kept so the key can be persisted in the C# provider's format.
    private: RsaPrivateKey,
}

impl std::fmt::Debug for DevKeyMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DevKeyMaterial")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl DevKeyMaterial {
    /// Loads the key at `path`, generating and persisting one if absent.
    ///
    /// A file that cannot be parsed is replaced rather than fatal, matching the
    /// original, which swallows JSON, cryptographic and format errors and
    /// regenerates.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if a key can be neither loaded nor generated.
    pub fn load_or_create(path: &Path) -> Result<Self, KeyError> {
        if let Some(existing) = Self::try_load(path) {
            return Ok(existing);
        }

        let key = Self::generate()?;
        key.save(path)?;

        Ok(key)
    }

    /// Generates a key without touching the filesystem, for tests.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if generation or encoding fails.
    pub fn generate() -> Result<Self, KeyError> {
        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, KEY_SIZE_BITS).map_err(KeyError::Generate)?;

        // The original's kid is base64url of eight random bytes.
        let mut seed = [0_u8; 8];
        rand::RngCore::fill_bytes(&mut rng, &mut seed);
        let kid = URL_SAFE_NO_PAD.encode(seed);

        Self::from_private_key(&private, kid)
    }

    fn from_private_key(private: &RsaPrivateKey, kid: String) -> Result<Self, KeyError> {
        let pem = private
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|error| KeyError::Encoding(error.to_string()))?;

        let encoding = EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|error| KeyError::Encoding(error.to_string()))?;

        let public = RsaPublicKey::from(private);
        let modulus = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let exponent = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());

        let decoding = DecodingKey::from_rsa_components(&modulus, &exponent)
            .map_err(|error| KeyError::Encoding(error.to_string()))?;

        Ok(Self {
            kid,
            encoding,
            decoding,
            modulus,
            exponent,
            private: private.clone(),
        })
    }

    fn try_load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let persisted: PersistedKey = serde_json::from_str(&text).ok()?;
        let der = STANDARD.decode(persisted.private_key_pkcs8_base64).ok()?;
        let private = RsaPrivateKey::from_pkcs8_der(&der).ok()?;

        Self::from_private_key(&private, persisted.kid).ok()
    }

    /// Writes the key to `path` in the C# provider's format.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), KeyError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| KeyError::File {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let der = self
            .private
            .to_pkcs8_der()
            .map_err(|error| KeyError::Encoding(error.to_string()))?;

        let document = PersistedKey {
            kid: self.kid.clone(),
            private_key_pkcs8_base64: STANDARD.encode(der.as_bytes()),
        };

        let json = serde_json::to_string_pretty(&document)
            .map_err(|error| KeyError::Encoding(error.to_string()))?;

        std::fs::write(path, json).map_err(|source| KeyError::File {
            path: path.to_path_buf(),
            source,
        })
    }
}

impl KeyMaterial for DevKeyMaterial {
    fn signing_key(&self) -> &EncodingKey {
        &self.encoding
    }

    fn validation_key(&self) -> &DecodingKey {
        &self.decoding
    }

    fn current_key_id(&self) -> &str {
        &self.kid
    }

    /// The JWKS document, matching the original's field order and values.
    ///
    /// One key only: the original publishes no overlap, so a rotation would
    /// invalidate every live token.
    fn jwks(&self) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": self.kid,
                "n": self.modulus,
                "e": self.exponent
            }]
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_generated_key_publishes_a_usable_jwks() {
        let key = DevKeyMaterial::generate().expect("a key should generate");

        let jwks = key.jwks();
        let entry = &jwks["keys"][0];

        assert_eq!(entry["kty"], "RSA");
        assert_eq!(entry["use"], "sig");
        assert_eq!(entry["alg"], "RS256");
        assert_eq!(entry["kid"], key.current_key_id());
        assert!(entry["n"].as_str().is_some_and(|n| n.len() > 300));
        // 65537, base64url of 0x010001.
        assert_eq!(entry["e"], "AQAB");
    }

    #[test]
    fn the_key_id_looks_like_the_originals() {
        // base64url of eight bytes, unpadded: eleven characters.
        let key = DevKeyMaterial::generate().expect("a key should generate");

        assert_eq!(key.current_key_id().len(), 11);
        assert!(!key.current_key_id().contains('='));
    }

    #[test]
    fn two_generated_keys_differ() {
        let first = DevKeyMaterial::generate().expect("a key should generate");
        let second = DevKeyMaterial::generate().expect("a key should generate");

        assert_ne!(first.current_key_id(), second.current_key_id());
    }
}
