use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wsl_code_switch_lib::adapters::FixedSyncCryptoEngine;
use wsl_code_switch_lib::domain::{
    PortableDomain, PortableRecordId, SyncCiphertext, SyncEncryptedEnvelope, SyncKdfProfile,
    SyncObjectIdentity,
};
use wsl_code_switch_lib::ports::{
    SyncCryptoError, SyncCryptoErrorCode, SyncCryptoPort, SyncCryptoRandom,
};

#[derive(Clone)]
struct SequenceRandom {
    state: Arc<Mutex<u8>>,
}

impl SequenceRandom {
    fn starting_at(value: u8) -> Self {
        Self {
            state: Arc::new(Mutex::new(value)),
        }
    }
}

impl SyncCryptoRandom for SequenceRandom {
    fn fill_bytes(&self, destination: &mut [u8]) -> Result<(), SyncCryptoError> {
        let mut next = self.state.lock().expect("sequence random lock");
        for byte in destination {
            *byte = *next;
            *next = next.wrapping_add(1);
        }
        Ok(())
    }
}

fn engine() -> FixedSyncCryptoEngine<SequenceRandom> {
    FixedSyncCryptoEngine::new(SequenceRandom::starting_at(0))
}

fn record_identity(key: &str, version: u64) -> SyncObjectIdentity {
    SyncObjectIdentity::record(
        PortableRecordId::new(PortableDomain::Provider, key).unwrap(),
        version,
    )
    .unwrap()
}

fn encrypted_fixture() -> (
    FixedSyncCryptoEngine<SequenceRandom>,
    SyncKdfProfile,
    SyncObjectIdentity,
    SyncEncryptedEnvelope,
) {
    let engine = engine();
    let profile = engine.create_profile().unwrap();
    let identity = record_identity("provider-a", 7);
    let session = engine
        .unlock(b"correct horse battery staple", &profile)
        .unwrap();
    let envelope = session
        .seal(&identity, b"P6-02-PLAINTEXT-SENTINEL")
        .unwrap();
    (engine, profile, identity, envelope)
}

#[test]
fn kdf_envelope_and_aad_versions_are_strict_and_self_describing() {
    let (_, profile, _, envelope) = encrypted_fixture();
    assert_eq!(profile.version_number(), 1);
    assert_eq!(profile.argon2_version(), 0x13);
    assert_eq!(profile.memory_kib(), 65_536);
    assert_eq!(profile.iterations(), 3);
    assert_eq!(profile.parallelism(), 1);
    assert_eq!(profile.output_length(), 32);

    let encoded = envelope.to_json_bytes().unwrap();
    let value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["envelopeVersion"], 1);
    assert_eq!(value["cipher"], "aes_256_gcm");
    assert_eq!(value["kdf"]["kdfVersion"], 1);
    assert_eq!(value["kdf"]["algorithm"], "argon2id");
    assert_eq!(value["identity"]["aadVersion"], 1);
    assert_eq!(value["identity"]["protocolVersion"], 3);
    assert_eq!(value["identity"]["objectType"], "record");
    assert_eq!(value["identity"]["objectVersion"], 7);
    assert_eq!(value["identity"]["recordId"]["domain"], "provider");
    assert_eq!(value["identity"]["recordId"]["key"], "provider-a");
    assert_eq!(
        SyncEncryptedEnvelope::from_json_bytes(&encoded).unwrap(),
        envelope
    );

    for (pointer, replacement) in [
        ("/envelopeVersion", json!(2)),
        ("/kdf/kdfVersion", json!(2)),
        ("/kdf/argon2Version", json!(0x10)),
        ("/identity/aadVersion", json!(2)),
        ("/identity/protocolVersion", json!(4)),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            SyncEncryptedEnvelope::from_json_bytes(&serde_json::to_vec(&changed).unwrap()).is_err()
        );
    }

    let mut unknown = value;
    unknown["plaintext"] = json!("must be rejected");
    assert!(
        SyncEncryptedEnvelope::from_json_bytes(&serde_json::to_vec(&unknown).unwrap()).is_err()
    );
}

#[test]
fn deterministic_known_answer_roundtrips_without_serializing_secrets() {
    let (engine, profile, identity, envelope) = encrypted_fixture();
    assert_eq!(profile.salt_base64(), "AAECAwQFBgcICQoLDA0ODw==");
    assert_eq!(envelope.nonce_base64(), "EBESExQVFhcYGRob");
    assert_eq!(
        envelope.ciphertext_base64(),
        "TFsgCRNipdS0KwVUAuU9Wx9PS19GDIDACRYyEXqMZG6J9EtobWgU1g=="
    );

    let encoded = envelope.to_json_bytes().unwrap();
    let encoded_text = String::from_utf8(encoded).unwrap();
    assert!(!encoded_text.contains("correct horse battery staple"));
    assert!(!encoded_text.contains("P6-02-PLAINTEXT-SENTINEL"));

    let session = engine
        .unlock(b"correct horse battery staple", &profile)
        .unwrap();
    assert_eq!(format!("{session:?}"), "FixedSyncCryptoSession([redacted])");
    let opened = session.open(&identity, &envelope).unwrap();
    assert_eq!(opened.as_bytes(), b"P6-02-PLAINTEXT-SENTINEL");
    let opened_debug = format!("{opened:?}");
    assert!(opened_debug.starts_with("SyncPlaintext"));
    assert!(!opened_debug.contains("P6-02-PLAINTEXT-SENTINEL"));

    let second = session.seal(&identity, b"same identity").unwrap();
    assert_ne!(second.nonce_base64(), envelope.nonce_base64());
}

#[test]
fn wrong_passphrase_and_tampering_fail_without_returning_plaintext() {
    let (engine, profile, identity, envelope) = encrypted_fixture();

    let wrong = engine.unlock(b"wrong passphrase", &profile).unwrap();
    assert_eq!(
        wrong.open(&identity, &envelope).unwrap_err().code,
        SyncCryptoErrorCode::AuthenticationFailed
    );

    let mut ciphertext = envelope.ciphertext().as_bytes().to_vec();
    ciphertext[0] ^= 0x80;
    let tampered = envelope
        .with_ciphertext(SyncCiphertext::new(ciphertext).unwrap())
        .unwrap();
    let correct = engine
        .unlock(b"correct horse battery staple", &profile)
        .unwrap();
    assert_eq!(
        correct.open(&identity, &tampered).unwrap_err().code,
        SyncCryptoErrorCode::AuthenticationFailed
    );

    let original: Value = serde_json::from_slice(&envelope.to_json_bytes().unwrap()).unwrap();
    for (pointer, replacement) in [
        ("/nonce", json!("ERESExQVFhcYGRob")),
        ("/kdf/salt", json!("AQECAwQFBgcICQoLDA0ODw==")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let changed =
            SyncEncryptedEnvelope::from_json_bytes(&serde_json::to_vec(&changed).unwrap()).unwrap();
        let changed_profile = changed.kdf().clone();
        let changed_session = engine
            .unlock(b"correct horse battery staple", &changed_profile)
            .unwrap();
        assert_eq!(
            changed_session
                .open(changed.identity(), &changed)
                .unwrap_err()
                .code,
            SyncCryptoErrorCode::AuthenticationFailed
        );
    }
}

#[test]
fn expected_object_type_record_id_and_version_are_enforced_before_open() {
    let (engine, profile, identity, envelope) = encrypted_fixture();
    let session = engine
        .unlock(b"correct horse battery staple", &profile)
        .unwrap();

    for wrong_identity in [
        SyncObjectIdentity::manifest(7).unwrap(),
        record_identity("provider-b", 7),
        record_identity("provider-a", 8),
    ] {
        assert_eq!(
            session.open(&wrong_identity, &envelope).unwrap_err().code,
            SyncCryptoErrorCode::IdentityMismatch
        );
    }

    assert_eq!(
        session.open(&identity, &envelope).unwrap().as_bytes(),
        b"P6-02-PLAINTEXT-SENTINEL"
    );
}

#[test]
fn tampered_clear_identity_is_authenticated_even_when_caller_accepts_it() {
    let (engine, profile, _, envelope) = encrypted_fixture();
    let mut value: Value = serde_json::from_slice(&envelope.to_json_bytes().unwrap()).unwrap();
    value["identity"]["recordId"]["key"] = json!("provider-b");
    let tampered =
        SyncEncryptedEnvelope::from_json_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
    let accepted_tampered_identity = tampered.identity().clone();
    let session = engine
        .unlock(b"correct horse battery staple", &profile)
        .unwrap();
    assert_eq!(
        session
            .open(&accepted_tampered_identity, &tampered)
            .unwrap_err()
            .code,
        SyncCryptoErrorCode::AuthenticationFailed
    );
}

#[test]
fn empty_or_oversized_passphrases_fail_before_crypto_work() {
    let engine = engine();
    let profile = engine.create_profile().unwrap();
    for passphrase in [Vec::new(), vec![b'x'; 1025]] {
        assert_eq!(
            engine.unlock(&passphrase, &profile).unwrap_err().code,
            SyncCryptoErrorCode::InvalidInput
        );
    }
}
