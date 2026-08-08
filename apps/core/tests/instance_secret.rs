use mediaflow_core::platform::secrets::{
    INSTANCE_KEY_BYTES, InstanceKey, IntegrationKind, SealedSecret, SecretAad, SecretCipher,
};
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn instance_key_is_created_once_with_exact_bytes_and_private_mode() {
    let root = tempdir().unwrap();
    let first = InstanceKey::load_or_create(root.path()).unwrap();
    let bytes = std::fs::read(root.path().join("instance.key")).unwrap();
    assert_eq!(bytes.len(), INSTANCE_KEY_BYTES);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(root.path().join("instance.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    let second = InstanceKey::load_or_create(root.path()).unwrap();
    let aad = SecretAad::new(Uuid::now_v7(), IntegrationKind::Tmdb, 1, 1);
    let sealed = first.seal(&aad, b"same-key-proof").unwrap();
    assert_eq!(
        second.open(&aad, &sealed).unwrap().expose(),
        b"same-key-proof"
    );
    assert!(!format!("{first:?}").contains(&hex::encode(bytes)));
}

#[test]
fn instance_key_rejects_short_files_and_final_symlinks_without_clobbering() {
    let short = tempdir().unwrap();
    std::fs::write(short.path().join("instance.key"), b"short").unwrap();
    assert!(InstanceKey::load_or_create(short.path()).is_err());
    assert_eq!(
        std::fs::read(short.path().join("instance.key")).unwrap(),
        b"short"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let linked = tempdir().unwrap();
        let target = linked.path().join("target.key");
        std::fs::write(&target, [7_u8; INSTANCE_KEY_BYTES]).unwrap();
        symlink(&target, linked.path().join("instance.key")).unwrap();
        assert!(InstanceKey::load_or_create(linked.path()).is_err());
        assert_eq!(std::fs::read(target).unwrap(), [7_u8; INSTANCE_KEY_BYTES]);

        let linked = tempdir().unwrap();
        let key = InstanceKey::load_or_create(linked.path()).unwrap();
        drop(key);
        std::fs::hard_link(
            linked.path().join("instance.key"),
            linked.path().join("instance.key.backup"),
        )
        .unwrap();
        assert!(InstanceKey::load_or_create(linked.path()).is_err());
        assert_eq!(
            std::fs::read(linked.path().join("instance.key")).unwrap(),
            std::fs::read(linked.path().join("instance.key.backup")).unwrap()
        );
    }
}

#[test]
fn xchacha_cipher_uses_unique_nonces_and_binds_all_aad_fields() {
    let root = tempdir().unwrap();
    let key = InstanceKey::load_or_create(root.path()).unwrap();
    let integration_id = Uuid::now_v7();
    let aad = SecretAad::new(integration_id, IntegrationKind::Tmdb, 7, 1);
    let plaintext = b"tmdb-secret-sentinel";

    let first = key.seal(&aad, plaintext).unwrap();
    let second = key.seal(&aad, plaintext).unwrap();
    assert_ne!(first.nonce(), second.nonce());
    assert_ne!(first.ciphertext(), second.ciphertext());
    assert_eq!(key.open(&aad, &first).unwrap().expose(), plaintext);
    assert!(!format!("{first:?}").contains("tmdb-secret-sentinel"));

    let mut tampered = first.ciphertext().to_vec();
    tampered[0] ^= 1;
    let tampered =
        SealedSecret::from_parts(first.schema_version(), *first.nonce(), tampered).unwrap();
    assert!(key.open(&aad, &tampered).is_err());
    assert!(
        key.open(
            &SecretAad::new(Uuid::now_v7(), IntegrationKind::Tmdb, 7, 1),
            &first,
        )
        .is_err()
    );
    assert!(
        key.open(
            &SecretAad::new(integration_id, IntegrationKind::Tmdb, 8, 1),
            &first,
        )
        .is_err()
    );
    assert!(
        key.open(
            &SecretAad::new(integration_id, IntegrationKind::Tmdb, 7, 2),
            &first,
        )
        .is_err()
    );
}
