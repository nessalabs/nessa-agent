//! Bounded inline authoring exchange. The package verifier still owns the bytes.

use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::STANDARD, Engine};

use crate::{
    scaffold_package, verify_package, AssetIntegrityError, PackageError, VerifiedPackage,
    MAX_PATH_BYTES,
};

const MAX_INLINE_BYTES: usize = 16 * 1024;
const MAX_INLINE_ASSETS: usize = 8;
const MAX_ENCODED_BYTES: usize = MAX_INLINE_BYTES.div_ceil(3) * 4;

/// One held asset in the inline authoring profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineAsset {
    pub path: String,
    pub bytes_base64: String,
}

/// Held bytes for a small static app. This is not an installed app or a grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlinePackage {
    pub manifest_base64: String,
    pub assets: Vec<InlineAsset>,
}

/// Validation facts about the supplied package, not evidence of provenance.
#[derive(Debug, Eq, PartialEq)]
pub struct ValidationReceipt {
    pub app_id: String,
    pub package_version: String,
    pub entrypoint: String,
    pub asset_count: usize,
}

/// Stable authoring input refusals; none retain caller-provided bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineRefusal {
    InvalidEncoding,
    InlineLimit,
    ManifestInvalid,
    AssetInventory,
    AssetLength,
    AssetDigest,
}

impl From<PackageError> for InlineRefusal {
    fn from(error: PackageError) -> Self {
        match error {
            PackageError::TooManyFiles
            | PackageError::MissingAsset
            | PackageError::UndeclaredAsset => Self::AssetInventory,
            PackageError::AssetIntegrity(AssetIntegrityError::LengthMismatch) => Self::AssetLength,
            PackageError::AssetIntegrity(AssetIntegrityError::DigestMismatch) => Self::AssetDigest,
            PackageError::ManifestTooLarge
            | PackageError::MalformedManifest
            | PackageError::UnsupportedVersion(_)
            | PackageError::InvalidAppId(_)
            | PackageError::InvalidPackageVersion
            | PackageError::InvalidRequestedCapabilities
            | PackageError::InvalidPath
            | PackageError::ConflictingPaths
            | PackageError::InvalidEntrypoint
            | PackageError::InvalidIcon
            | PackageError::InvalidDigest => Self::ManifestInvalid,
        }
    }
}

/// Build and encode the same verified static starter as the package owner.
///
/// # Errors
/// Rejects invalid declarations or content outside the small inline profile.
pub fn scaffold_inline_package(
    app_id: String,
    package_version: String,
) -> Result<InlinePackage, InlineRefusal> {
    checked_total(app_id.len(), package_version.len())?;
    encode_inline_package(&scaffold_package(app_id, package_version)?)
}

/// Encode verified bytes in the inline profile, as `apps.read` hands an
/// installed package back. A package larger than the profile is refused
/// rather than cut.
///
/// # Errors
/// Rejects a package outside the small inline profile.
pub fn encode_inline_package(package: &VerifiedPackage) -> Result<InlinePackage, InlineRefusal> {
    let mut total = package.manifest().json().len();
    let mut count = 0;
    for (_, bytes) in package.assets() {
        total = checked_total(total, bytes.len())?;
        count += 1;
    }
    if count > MAX_INLINE_ASSETS {
        return Err(InlineRefusal::InlineLimit);
    }
    Ok(InlinePackage {
        manifest_base64: STANDARD.encode(package.manifest().json()),
        assets: package
            .assets()
            .map(|(path, bytes)| InlineAsset {
                path: path.to_owned(),
                bytes_base64: STANDARD.encode(bytes),
            })
            .collect(),
    })
}

/// Decode and verify the exact held bytes, keeping them for a caller that
/// installs or renders what it checked.
///
/// # Errors
/// Rejects invalid encoding, oversize input, and nonverifying packages.
pub fn verify_inline_package(input: &InlinePackage) -> Result<VerifiedPackage, InlineRefusal> {
    verify_inline(input)
}

/// Validate the exact held bytes and return only the verification facts.
///
/// # Errors
/// Rejects invalid encoding, oversize input, and nonverifying packages.
pub fn validate_inline_package(input: &InlinePackage) -> Result<ValidationReceipt, InlineRefusal> {
    let package = verify_inline(input)?;
    let manifest = package.manifest();
    Ok(ValidationReceipt {
        app_id: manifest.app_id().as_str().to_owned(),
        package_version: manifest.package_version().to_owned(),
        entrypoint: manifest.entrypoint().to_owned(),
        asset_count: package.assets().count(),
    })
}

fn verify_inline(input: &InlinePackage) -> Result<VerifiedPackage, InlineRefusal> {
    if input.assets.len() > MAX_INLINE_ASSETS {
        return Err(InlineRefusal::InlineLimit);
    }
    let mut total = decoded_size(&input.manifest_base64)?;
    let mut labels = BTreeSet::new();
    for asset in &input.assets {
        if asset.path.len() > MAX_PATH_BYTES || !labels.insert(asset.path.as_str()) {
            return Err(InlineRefusal::AssetInventory);
        }
        total = checked_total(total, decoded_size(&asset.bytes_base64)?)?;
    }
    let manifest = decode(&input.manifest_base64)?;
    let held = input
        .assets
        .iter()
        .map(|asset| Ok((asset.path.clone(), decode(&asset.bytes_base64)?)))
        .collect::<Result<BTreeMap<_, _>, InlineRefusal>>()?;
    Ok(verify_package(manifest, held)?)
}

fn checked_total(left: usize, right: usize) -> Result<usize, InlineRefusal> {
    left.checked_add(right)
        .filter(|sum| *sum <= MAX_INLINE_BYTES)
        .ok_or(InlineRefusal::InlineLimit)
}

fn decoded_size(text: &str) -> Result<usize, InlineRefusal> {
    let len = text.len();
    if len > MAX_ENCODED_BYTES {
        return Err(InlineRefusal::InlineLimit);
    }
    if len % 4 != 0 {
        return Err(InlineRefusal::InvalidEncoding);
    }
    let padding = if text.ends_with("==") {
        2
    } else {
        usize::from(text.ends_with('='))
    };
    checked_total(len / 4 * 3 - padding, 0)
}

fn decode(text: &str) -> Result<Vec<u8>, InlineRefusal> {
    // STANDARD requires canonical padding and rejects nonzero trailing bits.
    STANDARD
        .decode(text)
        .map_err(|_| InlineRefusal::InvalidEncoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter() -> InlinePackage {
        scaffold_inline_package("sample-app".to_owned(), "preview + 1".to_owned()).unwrap()
    }

    #[test]
    fn verified_bytes_encode_back_to_the_same_package_and_digest() {
        let input = starter();
        let verified = verify_inline_package(&input).unwrap();
        assert_eq!(encode_inline_package(&verified), Ok(input.clone()));
        let digest = verified.digest();
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, verify_inline_package(&input).unwrap().digest());
        let other = verify_inline_package(
            &scaffold_inline_package("sample-app".to_owned(), "2".to_owned()).unwrap(),
        )
        .unwrap();
        assert_ne!(
            other.digest(),
            digest,
            "another manifest is another package"
        );
    }

    #[test]
    fn exact_starter_is_deterministic_and_reuses_package_validation() {
        let input = starter();
        assert_eq!(input, starter());
        let checked = verify_inline(&input).unwrap();
        assert_eq!(
            checked.manifest().json(),
            STANDARD.decode(&input.manifest_base64).unwrap()
        );
        assert_eq!(
            checked.asset("index.html").unwrap(),
            STANDARD.decode(&input.assets[0].bytes_base64).unwrap()
        );
        assert!(checked.manifest().requested_capabilities().is_empty());
        assert_eq!(
            validate_inline_package(&input),
            Ok(ValidationReceipt {
                app_id: "sample-app".to_owned(),
                package_version: "preview + 1".to_owned(),
                entrypoint: "index.html".to_owned(),
                asset_count: 1,
            })
        );

        assert_eq!(
            scaffold_inline_package("nessa".to_owned(), "preview".to_owned()),
            Err(InlineRefusal::ManifestInvalid)
        );
        assert_eq!(
            scaffold_inline_package("sample-app".to_owned(), " ".to_owned()),
            Err(InlineRefusal::ManifestInvalid)
        );
    }

    #[test]
    fn preserves_original_manifest_and_classifies_asset_failures() {
        let mut input = starter();
        let asset = STANDARD.decode(&input.assets[0].bytes_base64).unwrap();
        let mut json = STANDARD.decode(&input.manifest_base64).unwrap();
        json.push(b'\n');
        input.manifest_base64 = STANDARD.encode(&json);
        assert_eq!(verify_inline(&input).unwrap().manifest().json(), json);
        input.assets[0].bytes_base64 = STANDARD.encode(vec![0; asset.len()]);
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::AssetDigest)
        );
        input.assets[0].bytes_base64.clear();
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::AssetLength)
        );
        input.assets[0].path = "other.html".to_owned();
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::AssetInventory)
        );
        input.assets.clear();
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::AssetInventory)
        );
    }

    #[test]
    fn refuses_duplicate_labels_before_collecting_assets() {
        let mut input = starter();
        input.assets.push(InlineAsset {
            path: "index.html".to_owned(),
            bytes_base64: String::new(),
        });
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::AssetInventory)
        );
    }

    #[test]
    fn bounds_aggregate_decoded_bytes_and_number_of_assets() {
        let mut input = starter();
        input.manifest_base64 = STANDARD.encode(vec![0; MAX_INLINE_BYTES]);
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::InlineLimit)
        );
        input.manifest_base64.clear();
        input.assets = (0..=MAX_INLINE_ASSETS)
            .map(|index| InlineAsset {
                path: format!("{index}.html"),
                bytes_base64: String::new(),
            })
            .collect();
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::InlineLimit)
        );
        assert_eq!(
            scaffold_inline_package("sample-app".to_owned(), "v".repeat(MAX_INLINE_BYTES)),
            Err(InlineRefusal::InlineLimit)
        );
    }

    #[test]
    fn distinguishes_noncanonical_encoding_from_bad_manifest() {
        let mut input = starter();
        for encoded in ["Zh==", "Zg", "Zg=_"] {
            input.manifest_base64 = encoded.to_owned();
            assert_eq!(
                validate_inline_package(&input),
                Err(InlineRefusal::InvalidEncoding)
            );
        }
        input.manifest_base64 = STANDARD.encode(b"not a manifest");
        assert_eq!(
            validate_inline_package(&input),
            Err(InlineRefusal::ManifestInvalid)
        );
    }
}
