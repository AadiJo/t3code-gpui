//! RFC 9449 DPoP proofs signed with this install's P-256 key.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::platform::{SecretStore, SecretStoreError};

/// Secret store key holding the PKCS#8 private key.
const DPOP_KEY_SECRET: &str = "dpop-key";
/// The pairing code alphabet, which verification codes map into.
const PAIRING_CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const BASE64URL_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Debug, thiserror::Error)]
pub enum DpopKeyError {
    #[error(transparent)]
    Store(#[from] SecretStoreError),
    #[error("the stored DPoP key is invalid: {0}")]
    InvalidKey(String),
}

/// The public half as a JWK, members in RFC 7638 order.
#[derive(Serialize, Clone, Debug)]
pub struct PublicJwk {
    pub crv: &'static str,
    pub kty: &'static str,
    pub x: String,
    pub y: String,
}

/// This install's DPoP key. One per install, persisted in the secret store:
/// sessions minted with it are bound to it, so losing it means re-pairing.
#[derive(Clone)]
pub struct DpopKey {
    inner: Arc<KeyInner>,
}

struct KeyInner {
    pair: EcdsaKeyPair,
    jwk: PublicJwk,
    thumbprint: String,
    rng: SystemRandom,
}

#[derive(Serialize)]
struct ProofHeader<'a> {
    typ: &'static str,
    alg: &'static str,
    jwk: &'a PublicJwk,
}

#[derive(Serialize)]
struct ProofPayload<'a> {
    htm: &'a str,
    htu: &'a str,
    jti: String,
    iat: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ath: Option<String>,
}

impl DpopKey {
    /// Loads the key from `secrets`, creating and saving one on first use.
    pub fn load_or_create(secrets: &dyn SecretStore) -> Result<Self, DpopKeyError> {
        let rng = SystemRandom::new();
        let pkcs8 = match secrets.get(DPOP_KEY_SECRET)? {
            Some(encoded) => STANDARD
                .decode(&encoded)
                .map_err(|error| DpopKeyError::InvalidKey(error.to_string()))?,
            None => {
                let created = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .map_err(|error| DpopKeyError::InvalidKey(error.to_string()))?;
                secrets.set(DPOP_KEY_SECRET, STANDARD.encode(created.as_ref()).as_bytes())?;
                created.as_ref().to_vec()
            }
        };
        Self::from_pkcs8(&pkcs8)
    }

    fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, DpopKeyError> {
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
            .map_err(|error| DpopKeyError::InvalidKey(error.to_string()))?;
        // Uncompressed SEC1 point: 0x04 || x(32) || y(32).
        let point = pair.public_key().as_ref();
        let jwk = PublicJwk {
            crv: "P-256",
            kty: "EC",
            x: URL_SAFE_NO_PAD.encode(&point[1..33]),
            y: URL_SAFE_NO_PAD.encode(&point[33..65]),
        };
        let thumbprint = sha256_base64url(serde_json::to_string(&jwk).expect("jwk serializes").as_bytes());
        Ok(Self { inner: Arc::new(KeyInner { pair, jwk, thumbprint, rng }) })
    }

    /// RFC 7638 thumbprint (`jkt`) of the public key.
    pub fn thumbprint(&self) -> &str {
        &self.inner.thumbprint
    }

    pub fn public_jwk(&self) -> &PublicJwk {
        &self.inner.jwk
    }

    /// A fresh single-use proof for `method url`. `access_token` adds `ath`;
    /// for the socket upgrade pass the ticket, not the access token.
    pub fn proof(&self, method: &str, url: &Url, access_token: Option<&str>) -> String {
        let mut htu = url.clone();
        htu.set_query(None);
        htu.set_fragment(None);
        let header = ProofHeader { typ: "dpop+jwt", alg: "ES256", jwk: &self.inner.jwk };
        let payload = ProofPayload {
            htm: method,
            htu: htu.as_str(),
            jti: uuid::Uuid::new_v4().to_string(),
            iat: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            ath: access_token.map(|token| sha256_base64url(token.as_bytes())),
        };
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header serializes")),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload serializes")),
        );
        // FIXED signing yields the raw 64-byte r || s that JWS ES256 wants.
        let signature = self
            .inner
            .pair
            .sign(&self.inner.rng, signing_input.as_bytes())
            .expect("P-256 signing does not fail");
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
    }

    /// The `XXXX-XXXX` code the host shows while approving this device.
    pub fn verification_code(&self) -> String {
        verification_code(self.thumbprint())
    }
}

/// Maps the first 8 thumbprint characters into the pairing alphabet.
pub fn verification_code(thumbprint: &str) -> String {
    let code: String = thumbprint
        .bytes()
        .take(8)
        .map(|byte| {
            let value = BASE64URL_ALPHABET.iter().position(|&c| c == byte).unwrap_or(0);
            PAIRING_CODE_ALPHABET[value % PAIRING_CODE_ALPHABET.len()] as char
        })
        .collect();
    format!("{}-{}", &code[..4.min(code.len())], &code[4.min(code.len())..])
}

/// base64url(SHA-256(data)), no padding.
pub fn sha256_base64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(data))
}
