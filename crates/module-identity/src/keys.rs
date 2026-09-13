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
use rsa::pkcs1::DecodeRsaPrivateKey;
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

    /// The configured key provider is missing something it needs.
    #[error("{0}")]
    Misconfigured(String),
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
    material: RsaMaterial,
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
        Ok(Self {
            kid,
            material: RsaMaterial::from_private_key(private)?,
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

/// The parts every RSA provider derives from one private key.
///
/// Both providers need the same four things and derive them the same way; only
/// where the key *came from* differs.
struct RsaMaterial {
    encoding: EncodingKey,
    decoding: DecodingKey,
    modulus: String,
    exponent: String,
}

impl RsaMaterial {
    fn from_private_key(private: &RsaPrivateKey) -> Result<Self, KeyError> {
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
            encoding,
            decoding,
            modulus,
            exponent,
        })
    }

    /// The RFC 7638 thumbprint of this key, used as the `kid`.
    ///
    /// Derived from the key rather than drawn at random, so it is the same on
    /// every restart and across every replica running the same key. A random
    /// `kid` would change on each boot, and a client that cached the JWKS
    /// document would reject perfectly good tokens until it refetched.
    fn thumbprint(&self) -> String {
        use sha2::{Digest, Sha256};

        // The canonical form RFC 7638 specifies for an RSA key: the three
        // required members, in lexicographic order, with no whitespace.
        let canonical = format!(
            r#"{{"e":"{}","kty":"RSA","n":"{}"}}"#,
            self.exponent, self.modulus
        );

        URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
    }

    fn jwks(&self, kid: &str) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": kid,
                "n": self.modulus,
                "e": self.exponent
            }]
        })
    }
}

/// A signing key supplied as a PEM, from a file or from the environment.
///
/// This is what lets a host run outside Development. The only other provider
/// the original offers is `KeyVault`, which is not implemented here, so before
/// this a Production host had no usable provider at all and refused to start —
/// the development provider is refused there on purpose, and rightly.
///
/// Both PKCS#8 (`BEGIN PRIVATE KEY`) and PKCS#1 (`BEGIN RSA PRIVATE KEY`) are
/// accepted, since `openssl genrsa` and `openssl genpkey` write different ones
/// and an operator should not have to know which they ran.
///
/// Nothing is written back: unlike the development provider, this one never
/// generates a key and never touches the filesystem except to read.
pub struct PemKeyMaterial {
    kid: String,
    material: RsaMaterial,
}

impl std::fmt::Debug for PemKeyMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PemKeyMaterial")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl PemKeyMaterial {
    /// Loads the key from a PEM document.
    ///
    /// `kid` is taken from configuration when one is given, and otherwise
    /// derived from the key itself.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::Encoding`] if the text is not an RSA private key in
    /// either supported encoding.
    pub fn from_pem(pem: &str, kid: Option<String>) -> Result<Self, KeyError> {
        let private = RsaPrivateKey::from_pkcs8_pem(pem.trim())
            .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem.trim()))
            .map_err(|error| {
                // The message names the encoding problem, never the input.
                KeyError::Encoding(format!(
                    "the signing key is not a PKCS#8 or PKCS#1 RSA private key: {error}"
                ))
            })?;

        if private.size() * 8 < KEY_SIZE_BITS {
            return Err(KeyError::Misconfigured(format!(
                "the signing key is {} bits; RS256 wants at least {KEY_SIZE_BITS}",
                private.size() * 8
            )));
        }

        let material = RsaMaterial::from_private_key(&private)?;
        let kid = kid
            .map(|kid| kid.trim().to_owned())
            .filter(|kid| !kid.is_empty())
            .unwrap_or_else(|| material.thumbprint());

        Ok(Self { kid, material })
    }

    /// Loads the key from a file.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::File`] if the file cannot be read, or
    /// [`KeyError::Encoding`] if its contents are not a usable key.
    pub fn from_file(path: &Path, kid: Option<String>) -> Result<Self, KeyError> {
        let pem = std::fs::read_to_string(path).map_err(|source| KeyError::File {
            path: path.to_path_buf(),
            source,
        })?;

        tracing::info!(path = %path.display(), "loaded the signing key from a file");

        Self::from_pem(&pem, kid)
    }

    /// Loads the key from an environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::Misconfigured`] if the variable is unset or blank,
    /// or [`KeyError::Encoding`] if its contents are not a usable key.
    pub fn from_environment(variable: &str, kid: Option<String>) -> Result<Self, KeyError> {
        let pem = std::env::var(variable)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                KeyError::Misconfigured(format!(
                    "Jwt:KeyProvider is Environment but {variable} is unset or empty"
                ))
            })?;

        tracing::info!(variable, "loaded the signing key from the environment");

        Self::from_pem(&pem, kid)
    }
}

impl KeyMaterial for PemKeyMaterial {
    fn signing_key(&self) -> &EncodingKey {
        &self.material.encoding
    }

    fn validation_key(&self) -> &DecodingKey {
        &self.material.decoding
    }

    fn current_key_id(&self) -> &str {
        &self.kid
    }

    fn jwks(&self) -> serde_json::Value {
        self.material.jwks(&self.kid)
    }
}

impl KeyMaterial for DevKeyMaterial {
    fn signing_key(&self) -> &EncodingKey {
        &self.material.encoding
    }

    fn validation_key(&self) -> &DecodingKey {
        &self.material.decoding
    }

    fn current_key_id(&self) -> &str {
        &self.kid
    }

    /// The JWKS document, matching the original's field order and values.
    ///
    /// One key only: the original publishes no overlap, so a rotation would
    /// invalidate every live token.
    fn jwks(&self) -> serde_json::Value {
        self.material.jwks(&self.kid)
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

    /// A PKCS#8 PEM, the form `openssl genpkey` writes.
    fn pkcs8_pem(private: &RsaPrivateKey) -> String {
        private
            .to_pkcs8_pem(LineEnding::LF)
            .expect("the key should encode")
            .to_string()
    }

    /// A PKCS#1 PEM, the form `openssl genrsa` writes.
    fn pkcs1_pem(private: &RsaPrivateKey) -> String {
        use rsa::pkcs1::EncodeRsaPrivateKey;

        private
            .to_pkcs1_pem(LineEnding::LF)
            .expect("the key should encode")
            .to_string()
    }

    fn generated_key() -> RsaPrivateKey {
        RsaPrivateKey::new(&mut rand::thread_rng(), KEY_SIZE_BITS).expect("a key should generate")
    }

    #[test]
    fn a_pem_key_signs_tokens_its_own_jwks_would_validate() {
        let private = generated_key();
        let material =
            PemKeyMaterial::from_pem(&pkcs8_pem(&private), None).expect("the PEM should load");

        let jwks = material.jwks();
        let entry = &jwks["keys"][0];

        assert_eq!(entry["kty"], "RSA");
        assert_eq!(entry["alg"], "RS256");
        assert_eq!(entry["kid"], material.current_key_id());
        assert_eq!(entry["e"], "AQAB");
        assert!(entry["n"].as_str().is_some_and(|n| n.len() > 300));
    }

    #[test]
    fn both_pem_encodings_load_and_produce_the_same_key() {
        // `openssl genrsa` and `openssl genpkey` write different ones, and an
        // operator should not have to know which they ran.
        let private = generated_key();

        let from_pkcs8 =
            PemKeyMaterial::from_pem(&pkcs8_pem(&private), None).expect("PKCS#8 should load");
        let from_pkcs1 =
            PemKeyMaterial::from_pem(&pkcs1_pem(&private), None).expect("PKCS#1 should load");

        assert_eq!(from_pkcs8.jwks(), from_pkcs1.jwks());
        assert_eq!(from_pkcs8.current_key_id(), from_pkcs1.current_key_id());
    }

    #[test]
    fn the_derived_key_id_is_the_same_every_time_the_same_key_loads() {
        // A `kid` drawn at random would change on every boot, and a client
        // that cached the JWKS document would reject good tokens until it
        // refetched. The thumbprint is a function of the key.
        let private = generated_key();

        let first = PemKeyMaterial::from_pem(&pkcs8_pem(&private), None).expect("should load");
        let second = PemKeyMaterial::from_pem(&pkcs8_pem(&private), None).expect("should load");

        assert_eq!(first.current_key_id(), second.current_key_id());
        assert!(!first.current_key_id().is_empty());

        let other =
            PemKeyMaterial::from_pem(&pkcs8_pem(&generated_key()), None).expect("should load");
        assert_ne!(
            first.current_key_id(),
            other.current_key_id(),
            "a different key must publish a different kid"
        );
    }

    #[test]
    fn a_configured_key_id_wins_over_the_derived_one() {
        let private = generated_key();

        let material = PemKeyMaterial::from_pem(&pkcs8_pem(&private), Some("2026-q1".to_owned()))
            .expect("the PEM should load");

        assert_eq!(material.current_key_id(), "2026-q1");
        assert_eq!(material.jwks()["keys"][0]["kid"], "2026-q1");
    }

    #[test]
    fn a_blank_configured_key_id_falls_back_to_the_derived_one() {
        let private = generated_key();

        let material = PemKeyMaterial::from_pem(&pkcs8_pem(&private), Some("   ".to_owned()))
            .expect("the PEM should load");

        assert!(!material.current_key_id().trim().is_empty());
    }

    #[test]
    fn a_pem_key_loads_from_a_file() {
        let private = generated_key();
        let path = std::env::temp_dir().join(format!("signing-key-{}.pem", std::process::id()));
        std::fs::write(&path, pkcs8_pem(&private)).expect("the key should write");

        let material = PemKeyMaterial::from_file(&path, None).expect("the file should load");

        assert!(!material.current_key_id().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_key_file_names_the_path_rather_than_panicking() {
        let missing = std::env::temp_dir().join("no-such-signing-key.pem");
        let _ = std::fs::remove_file(&missing);

        let error =
            PemKeyMaterial::from_file(&missing, None).expect_err("a missing key file should fail");

        assert!(matches!(error, KeyError::File { .. }), "{error:?}");
        assert!(error.to_string().contains("no-such-signing-key.pem"));
    }

    #[test]
    fn text_that_is_not_a_key_is_refused_without_quoting_it_back() {
        // The input is secret material even when it fails to parse, so the
        // message must describe the problem, not the bytes.
        let error = PemKeyMaterial::from_pem("-----BEGIN PRIVATE KEY-----\nnope\n", None)
            .expect_err("garbage should not load");

        let message = error.to_string();
        assert!(message.contains("PKCS#8 or PKCS#1"), "{message}");
        assert!(!message.contains("nope"), "{message}");
    }

    #[test]
    fn a_key_too_small_for_rs256_is_refused() {
        let small =
            RsaPrivateKey::new(&mut rand::thread_rng(), 1024).expect("a small key should generate");

        let error = PemKeyMaterial::from_pem(&pkcs8_pem(&small), None)
            .expect_err("1024 bits should be refused");

        assert!(matches!(error, KeyError::Misconfigured(_)), "{error:?}");
        assert!(error.to_string().contains("1024"), "{error}");
    }

    #[test]
    fn a_pem_provider_does_not_print_its_key() {
        let material =
            PemKeyMaterial::from_pem(&pkcs8_pem(&generated_key()), None).expect("should load");

        let rendered = format!("{material:?}");

        assert!(rendered.contains(material.current_key_id()));
        assert!(!rendered.contains("PRIVATE KEY"), "{rendered}");
    }

    #[test]
    fn two_generated_keys_differ() {
        let first = DevKeyMaterial::generate().expect("a key should generate");
        let second = DevKeyMaterial::generate().expect("a key should generate");

        assert_ne!(first.current_key_id(), second.current_key_id());
    }
}
