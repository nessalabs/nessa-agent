use nessa_app_package::{verify_asset, AssetIntegrityError};
use nessa_types::app_identity::ExtensionAppId;

// SHA-256 known-answer vector for the three ASCII bytes in "abc".
const ABC_SHA256: [u8; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];

#[test]
fn returns_the_exact_held_bytes_and_the_validated_app_id() {
    let app_id = ExtensionAppId::new("calendar").unwrap();
    let bytes = b"abc";
    let asset = verify_asset(&app_id, bytes, 3, ABC_SHA256).unwrap();
    assert_eq!(asset.app_id(), &app_id);
    assert_eq!(asset.bytes(), b"abc");
    assert_eq!(asset.bytes().as_ptr(), bytes.as_ptr());
}

#[test]
fn rejects_truncated_and_extended_bytes_as_length_mismatches() {
    let app_id = ExtensionAppId::new("calendar").unwrap();
    for bytes in [b"ab".as_slice(), b"abcd".as_slice()] {
        assert_eq!(
            verify_asset(&app_id, bytes, 3, ABC_SHA256).unwrap_err(),
            AssetIntegrityError::LengthMismatch
        );
    }
}

#[test]
fn rejects_same_length_tampering_as_a_digest_mismatch() {
    let app_id = ExtensionAppId::new("calendar").unwrap();
    assert_eq!(
        verify_asset(&app_id, b"abd", 3, ABC_SHA256).unwrap_err(),
        AssetIntegrityError::DigestMismatch
    );
}
