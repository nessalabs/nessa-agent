//! A resource ticket's spelling, against the protocol schema that publishes
//! it, and its digest.
use super::*;
use serde_json::Value;

/// The schema's `McpReadResourceResult.ticket`, as published.
fn schema_ticket() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    );
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    schema["$defs"]["McpReadResourceResult"]["properties"]["ticket"].clone()
}

/// The characters a `^[…]{n}$` pattern's one class admits, and `n`.
fn single_class(pattern: &str) -> (Vec<char>, usize) {
    let inner = pattern
        .strip_prefix("^[")
        .and_then(|rest| rest.strip_suffix('$'))
        .expect("a pattern of one character class");
    let (class, count) = inner.split_once("]{").expect("a counted class");
    let count = count.strip_suffix('}').unwrap().parse().unwrap();
    let chars: Vec<char> = class.chars().collect();
    let mut admitted = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        // A `-` between two characters is a range; anywhere else, itself.
        if index + 2 < chars.len() && chars[index + 1] == '-' {
            admitted.extend(chars[index]..=chars[index + 2]);
            index += 3;
        } else {
            admitted.push(chars[index]);
            index += 1;
        }
    }
    (admitted, count)
}

#[test]
fn a_ticket_is_43_characters_of_the_schemas_alphabet() {
    let schema = schema_ticket();
    let (alphabet, length) = single_class(schema["pattern"].as_str().unwrap());
    assert_eq!(schema["minLength"], length);
    assert_eq!(schema["maxLength"], length);
    for bytes in [
        [0u8; 32],
        [0xff; 32],
        std::array::from_fn(|i| (i as u8).wrapping_mul(37)),
    ] {
        let ticket = resource_ticket(bytes);
        assert_eq!(ticket.len(), length, "{ticket}");
        assert!(ticket.chars().all(|c| alphabet.contains(&c)), "{ticket}");
    }
    // All 256 bits are in it: distinct bytes, distinct tickets.
    let mut other = [0u8; 32];
    other[31] = 1;
    assert_ne!(resource_ticket([0; 32]), resource_ticket(other));
}

#[test]
fn a_digest_is_the_tickets_sha256_and_never_shows_the_ticket() {
    let ticket = resource_ticket([9; 32]);
    let digest = ResourceTicketDigest::of(&ticket);
    assert_eq!(digest, ResourceTicketDigest::of(ticket.as_bytes()));
    assert_eq!(digest.to_hex().len(), 64);
    assert!(digest
        .to_hex()
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    assert!(!format!("{digest:?}").contains(&ticket));
    assert_ne!(digest, ResourceTicketDigest::of(resource_ticket([8; 32])));
}
