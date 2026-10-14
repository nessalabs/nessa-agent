//! Which gateway paths the cut may copy. Anything else is fingerprinted.

use std::path::Path;

use crate::backup::domain::Sensitivity;

/// `path`'s sensitivity from the files the cut is allowed to copy.
///
/// Public files are the named store files (`leases.jsonl`, `grants.json`,
/// `grants-audit.jsonl`, `paired-with`) and a JSON record whose name is a
/// lowercase UUID, 64 hex digits, a hold (`64 hex` + `-` + `16 hex`), or
/// `retired-` plus 64 hex digits. Every other path is secret, including a
/// new key or token file whose name is not one of those.
pub fn path_sensitivity(path: &Path) -> Sensitivity {
    if public_store_file(path) {
        Sensitivity::Public
    } else {
        Sensitivity::Secret
    }
}

fn public_store_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        "leases.jsonl" | "grants.json" | "grants-audit.jsonl" | "paired-with"
    ) || public_json_record(name)
}

fn public_json_record(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    lowercase_hex(stem, 64)
        || lowercase_uuid(stem)
        || hold_record(stem)
        || stem
            .strip_prefix("retired-")
            .is_some_and(|id| lowercase_hex(id, 64))
}

/// `<digest>-<media tag>`, the hold record's name.
fn hold_record(stem: &str) -> bool {
    let Some((digest, tag)) = stem.split_once('-') else {
        return false;
    };
    lowercase_hex(digest, 64) && lowercase_hex(tag, 16)
}

fn lowercase_uuid(value: &str) -> bool {
    let mut parts = value.split('-');
    lowercase_hex(parts.next().unwrap_or(""), 8)
        && lowercase_hex(parts.next().unwrap_or(""), 4)
        && lowercase_hex(parts.next().unwrap_or(""), 4)
        && lowercase_hex(parts.next().unwrap_or(""), 4)
        && lowercase_hex(parts.next().unwrap_or(""), 12)
        && parts.next().is_none()
}

fn lowercase_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
