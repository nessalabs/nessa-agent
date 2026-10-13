//! Which gateway paths hold key material. Those bytes are fingerprinted,
//! not copied into a bundle.

use std::path::Path;

use crate::backup::domain::Sensitivity;

/// `path`'s sensitivity from the role of its name.
///
/// `gateway-key`, `client-enrollment`, `credentials.v1.json`, a `secrets`
/// directory, and `*.token` hold private keys, verifiers, or bearer tokens.
/// Every other path is public record data.
pub fn path_sensitivity(path: &Path) -> Sensitivity {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name == "gateway-key"
        || name == "client-enrollment"
        || name == "credentials.v1.json"
        || name.ends_with(".token")
        || path
            .components()
            .any(|component| component.as_os_str() == "secrets")
    {
        Sensitivity::Secret
    } else {
        Sensitivity::Public
    }
}
