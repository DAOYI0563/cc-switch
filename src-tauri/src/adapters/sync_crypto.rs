use std::fmt;
use std::sync::Arc;

use aes_gcm::aead::rand_core::{OsRng, RngCore};
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use crate::domain::{
    SyncCiphertext, SyncEncryptedEnvelope, SyncKdfProfile, SyncKdfSalt, SyncNonce,
    SyncObjectIdentity, MAX_SYNC_PLAINTEXT_BYTES, SYNC_GCM_NONCE_BYTES, SYNC_GCM_TAG_BYTES,
    SYNC_KDF_SALT_BYTES,
};
use crate::ports::{
    SyncCryptoError, SyncCryptoErrorCode, SyncCryptoPort, SyncCryptoRandom, SyncCryptoSession,
    SyncPlaintext,
};

const MAX_PASSPHRASE_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSyncCryptoRandom;

impl SyncCryptoRandom for SystemSyncCryptoRandom {
    fn fill_bytes(&self, destination: &mut [u8]) -> Result<(), SyncCryptoError> {
        OsRng.try_fill_bytes(destination).map_err(|_| {
            SyncCryptoError::new(
                SyncCryptoErrorCode::RandomnessFailed,
                "secure random generation failed",
            )
        })
    }
}

#[derive(Clone)]
pub struct FixedSyncCryptoEngine<R = SystemSyncCryptoRandom> {
    random: Arc<R>,
}

impl FixedSyncCryptoEngine<SystemSyncCryptoRandom> {
    pub fn runtime() -> Self {
        Self::new(SystemSyncCryptoRandom)
    }
}

impl<R> FixedSyncCryptoEngine<R> {
    pub fn new(random: R) -> Self {
        Self {
            random: Arc::new(random),
        }
    }
}

impl<R> fmt::Debug for FixedSyncCryptoEngine<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FixedSyncCryptoEngine")
    }
}

impl<R> SyncCryptoPort for FixedSyncCryptoEngine<R>
where
    R: SyncCryptoRandom + 'static,
{
    fn create_profile(&self) -> Result<SyncKdfProfile, SyncCryptoError> {
        let mut salt = [0_u8; SYNC_KDF_SALT_BYTES];
        self.random.fill_bytes(&mut salt)?;
        Ok(SyncKdfProfile::recommended(SyncKdfSalt::new(salt)))
    }

    fn unlock(
        &self,
        passphrase: &[u8],
        profile: &SyncKdfProfile,
    ) -> Result<Box<dyn SyncCryptoSession>, SyncCryptoError> {
        if passphrase.is_empty() || passphrase.len() > MAX_PASSPHRASE_BYTES {
            return Err(SyncCryptoError::new(
                SyncCryptoErrorCode::InvalidInput,
                "sync passphrase has an invalid length",
            ));
        }
        profile.validate().map_err(invalid_contract)?;
        let params = Params::new(
            profile.memory_kib(),
            profile.iterations(),
            profile.parallelism(),
            Some(profile.output_length() as usize),
        )
        .map_err(|_| key_derivation_error())?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = Zeroizing::new([0_u8; 32]);
        argon2
            .hash_password_into(passphrase, profile.salt().as_bytes(), key.as_mut())
            .map_err(|_| key_derivation_error())?;
        Ok(Box::new(FixedSyncCryptoSession {
            profile: profile.clone(),
            key,
            random: Arc::clone(&self.random),
        }))
    }
}

struct FixedSyncCryptoSession<R> {
    profile: SyncKdfProfile,
    key: Zeroizing<[u8; 32]>,
    random: Arc<R>,
}

impl<R> fmt::Debug for FixedSyncCryptoSession<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FixedSyncCryptoSession([redacted])")
    }
}

impl<R> SyncCryptoSession for FixedSyncCryptoSession<R>
where
    R: SyncCryptoRandom + 'static,
{
    fn seal(
        &self,
        identity: &SyncObjectIdentity,
        plaintext: &[u8],
    ) -> Result<SyncEncryptedEnvelope, SyncCryptoError> {
        identity.validate().map_err(invalid_contract)?;
        if plaintext.len() > MAX_SYNC_PLAINTEXT_BYTES {
            return Err(SyncCryptoError::new(
                SyncCryptoErrorCode::InvalidInput,
                "sync plaintext exceeds the object size limit",
            ));
        }
        let mut nonce_bytes = [0_u8; SYNC_GCM_NONCE_BYTES];
        self.random.fill_bytes(&mut nonce_bytes)?;
        let aad = SyncEncryptedEnvelope::authenticated_metadata_bytes(&self.profile, identity)
            .map_err(invalid_contract)?;
        let cipher =
            Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| encryption_error())?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let mut encrypted = plaintext.to_vec();
        let tag = cipher
            .encrypt_in_place_detached(nonce, &aad, &mut encrypted)
            .map_err(|_| encryption_error())?;
        encrypted.extend_from_slice(tag.as_slice());
        let ciphertext = SyncCiphertext::new(encrypted).map_err(invalid_contract)?;
        SyncEncryptedEnvelope::new(
            self.profile.clone(),
            identity.clone(),
            SyncNonce::new(nonce_bytes),
            ciphertext,
        )
        .map_err(invalid_contract)
    }

    fn open(
        &self,
        expected_identity: &SyncObjectIdentity,
        envelope: &SyncEncryptedEnvelope,
    ) -> Result<SyncPlaintext, SyncCryptoError> {
        expected_identity.validate().map_err(invalid_contract)?;
        envelope.validate().map_err(invalid_contract)?;
        if envelope.identity() != expected_identity {
            return Err(SyncCryptoError::new(
                SyncCryptoErrorCode::IdentityMismatch,
                "encrypted sync object identity does not match the expected identity",
            ));
        }
        if envelope.kdf() != &self.profile {
            return Err(SyncCryptoError::new(
                SyncCryptoErrorCode::ProfileMismatch,
                "encrypted sync object uses a different KDF profile",
            ));
        }
        let encrypted = envelope.ciphertext().as_bytes();
        let split = encrypted.len() - SYNC_GCM_TAG_BYTES;
        let (body, tag) = encrypted.split_at(split);
        let mut plaintext = Zeroizing::new(body.to_vec());
        let cipher =
            Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| encryption_error())?;
        let aad = SyncEncryptedEnvelope::authenticated_metadata_bytes(
            envelope.kdf(),
            envelope.identity(),
        )
        .map_err(invalid_contract)?;
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(envelope.nonce().as_bytes()),
                &aad,
                &mut plaintext,
                Tag::from_slice(tag),
            )
            .map_err(|_| authentication_error())?;
        Ok(SyncPlaintext::new(std::mem::take(&mut *plaintext)))
    }
}

fn invalid_contract(error: crate::domain::DomainError) -> SyncCryptoError {
    SyncCryptoError::new(SyncCryptoErrorCode::InvalidInput, error.to_string())
}

fn key_derivation_error() -> SyncCryptoError {
    SyncCryptoError::new(
        SyncCryptoErrorCode::KeyDerivationFailed,
        "sync key derivation failed",
    )
}

fn encryption_error() -> SyncCryptoError {
    SyncCryptoError::new(
        SyncCryptoErrorCode::EncryptionFailed,
        "sync object encryption failed",
    )
}

fn authentication_error() -> SyncCryptoError {
    SyncCryptoError::new(
        SyncCryptoErrorCode::AuthenticationFailed,
        "sync object authentication failed",
    )
}
