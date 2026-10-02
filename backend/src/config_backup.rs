//! Encrypted config backup.
//!
//! Three sections are coverable: channels (with `api_key`), tokens (with
//! `key`), and model mappings. `users` is deliberately not exported — there's
//! no business reason to migrate admin identities between deployments, and
//! shipping hashed passwords out of a system only widens the surface area.
//!
//! File format, all little-endian, version-tagged for forward compat:
//!
//! ```text
//!   bytes  0..4    magic "LRBA"
//!   byte   4       format version (currently 1)
//!   byte   5       flags (reserved — must be 0)
//!   bytes  6..22   PBKDF2 salt (16 random bytes)
//!   bytes 22..34   AES-GCM nonce (12 random bytes)
//!   bytes 34..    AES-GCM-256 ciphertext + 16-byte tag
//! ```
//!
//! Inside the ciphertext is a single JSON document:
//!
//! ```text
//! {
//!   "created_at":  1717000000,
//!   "sections":    ["channels", "tokens", "mappings"],
//!   "channels":    [...],
//!   "tokens":      [...],
//!   "mappings":    [...]
//! }
//! ```
//!
//! Channels and tokens are keyed by `name` (per-row conflict at preview time).
//! Mappings are keyed by `alias`.
//!
//! The auth-tag failure is the only decrypt — wrong passphrase and tampered
//! file both surface as `Decrypt("auth tag mismatch")`, never as "partial
//! success".

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use pbkdf2::pbkdf2_hmac_array;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::error::Error as StdError;
use std::fmt;

/// Format magic, ASCII "LRBA" (`L`iterouter `R`outer `BA`ckup).
const MAGIC: &[u8; 4] = b"LRBA";
const FORMAT_VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 4 + 1 + 1 + SALT_LEN + NONCE_LEN;
/// PBKDF2-HMAC-SHA256 iteration count. Matches the project's password-hash
/// factor so a stolen passphrase gets the same treatment everywhere.
const KDF_ITERS: u32 = 100_000;
/// Key length for AES-256.
const KEY_LEN: usize = 32;

#[derive(Debug)]
pub enum Exit {
    /// Header magic wrong — not a backup file produced by this gateway.
    NotABackup,
    /// Format version not understood.
    UnknownVersion(u8),
    /// AES-GCM auth tag check failed: the passphrase was wrong, or the
    /// ciphertext was tampered with. We can't tell those two apart on
    /// purpose — both surface the same error.
    Decrypt(String),
    /// The decrypted JSON didn't parse, or had an unexpected variant.
    EmptyBody(String),
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Exit::NotABackup => write!(f, "not a LiteRouter backup file"),
            Exit::UnknownVersion(v) => {
                write!(f, "backup format version {v} is not supported")
            }
            Exit::Decrypt(m) => write!(f, "decrypt failed: {m}"),
            Exit::EmptyBody(m) => write!(f, "backup body invalid: {m}"),
        }
    }
}

impl StdError for Exit {}

/// A snapshot of the three sections. Empty vectors are not serialized on
/// read (and mean "section not selected"), so a backup file with only
/// `channels` is exactly that — no `tokens`/`mappings` data even exists on
/// the source.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupPayload {
    /// Seconds since epoch on the source at export time. Diagnostic only.
    pub created_at: i64,
    /// Names of sections included in this backup. Used by the preview step
    /// to refuse "user asked to import tokens, file has none" — useful
    /// rather than silently importing an empty one.
    pub sections: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<TokenRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<MappingRow>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct ChannelRow {
    pub name: String,
    pub website: String,
    pub base_url: String,
    pub base_url_anthropic: String,
    pub api_key: String,
    /// Comma-separated list, matching `channels.models`.
    pub models: String,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct TokenRow {
    pub name: String,
    /// The sk-… credential. Owners only see their own; exporting a token
    /// includes its owner so importing can rebuild `tokens.user_id` on a
    /// foreign system (resolved by lookup; 400 if username missing).
    pub key: String,
    pub enabled: bool,
    pub rpm_limit: i64,
    pub daily_token_limit: i64,
    /// Username of the owning account. Resolved to `tokens.user_id` on
    /// import via a `users.username` lookup.
    pub username: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct MappingRow {
    pub alias: String,
    /// Each entry is a `(channel_name, model)` pair. The internal
    /// `target_model` legacy field is derived from `targets[0].model` on
    /// import, so it doesn't need to round-trip in the export.
    pub targets: Vec<MappingTarget>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct MappingTarget {
    pub channel: String,
    pub model: String,
}

/// Encrypt the payload with AES-256-GCM. The key is PBKDF2(passphrase, salt)
/// at 100k iters — same work factor as the project's password hashing. The
/// nonce is freshly random per encryption; reusing it under the same key
/// would leak the XOR of two messages.
pub fn encrypt(passphrase: &str, payload: &BackupPayload) -> Vec<u8> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce_bytes);
    let key = derive_key(passphrase, &salt);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let plaintext = serde_json::to_vec(payload).expect("backup payload serializes");
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .expect("AES-GCM encrypt cannot fail with a 12-byte nonce");
    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.push(0); // reserved
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    out
}

/// Reverse of `encrypt`. Auth-tag check is part of the API.
pub fn decrypt(passphrase: &str, blob: &[u8]) -> Result<BackupPayload, Exit> {
    if blob.len() < HEADER_LEN {
        return Err(Exit::Decrypt("file is too short".into()));
    }
    if &blob[0..4] != MAGIC {
        return Err(Exit::NotABackup);
    }
    let version = blob[4];
    if version != FORMAT_VERSION {
        return Err(Exit::UnknownVersion(version));
    }
    let salt: [u8; SALT_LEN] = blob[6..6 + SALT_LEN].try_into().unwrap();
    let nonce_bytes: [u8; NONCE_LEN] = blob[6 + SALT_LEN..HEADER_LEN].try_into().unwrap();
    let ciphertext = &blob[HEADER_LEN..];
    let key = derive_key(passphrase, &salt);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext)
        .map_err(|e| Exit::Decrypt(e.to_string()))?;
    serde_json::from_slice(&plaintext).map_err(|e| Exit::EmptyBody(e.to_string()))
}

/// Format a unix-second timestamp as `YYYY-MM-DD` for export filenames.
/// Calendar math, no chrono — the small surface area is the win.
pub fn format_stamp(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let mut y = 1970i64;
    let mut remaining = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let year_len = if leap { 366 } else { 365 };
        if remaining < year_len {
            break;
        }
        remaining -= year_len;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_lens = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0usize;
    for (i, len) in month_lens.iter().enumerate() {
        if remaining < *len {
            m = i;
            break;
        }
        remaining -= *len;
    }
    let day = remaining + 1;
    format!("{:04}-{:02}-{:02}", y, m + 1, day)
}

fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; KEY_LEN] {
    pbkdf2_hmac_array::<Sha256, KEY_LEN>(passphrase.as_bytes(), salt, KDF_ITERS)
}
