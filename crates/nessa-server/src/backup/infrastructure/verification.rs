//! Read-only adapters for an explicitly supplied private bundle and MAC key.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use nessa_local_storage::{
    is_unsafe_file, open, open_beneath, verify_directory, OpenMode, PrivateDirectory,
    PrivateFileType,
};
use sha2::Sha256;

use crate::backup::application::{
    MacKey, ManifestMacVerifier, VerificationFailure, VerificationSource,
};
use crate::backup::domain::{Fingerprint, StoreKind};

pub struct DirectoryVerificationSource {
    root: PathBuf,
}

impl DirectoryVerificationSource {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl ManifestMacVerifier for MacKey {
    fn verify(&self, canonical: &str, expected: Fingerprint) -> Result<(), VerificationFailure> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).map_err(|_| VerificationFailure::InvalidKey)?;
        mac.update(canonical.as_bytes());
        mac.verify_slice(expected.bytes())
            .map_err(|_| VerificationFailure::Authentication)
    }
}

/// Read exactly the existing key representation. No directory/key creation.
pub fn read_verification_key(path: &Path) -> Result<MacKey, VerificationFailure> {
    let file = open(path, OpenMode::ReadNonblocking).map_err(read_error)?;
    let mut bytes = Vec::new();
    file.take(33).read_to_end(&mut bytes).map_err(read_error)?;
    let bytes = bytes
        .try_into()
        .map_err(|_| VerificationFailure::InvalidKey)?;
    Ok(MacKey(bytes))
}

impl VerificationSource for DirectoryVerificationSource {
    fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        verify_directory(&self.root).map_err(read_error)?;
        let file = open_beneath(
            &self.root,
            Path::new("manifest.txt"),
            OpenMode::ReadNonblocking,
        )
        .map_err(read_error)?;
        Ok(Box::new(file))
    }

    fn record(
        &self,
        store: StoreKind,
        encoded_identity: &str,
    ) -> Result<Box<dyn Read + '_>, VerificationFailure> {
        let relative = Path::new("records").join(store.as_str());
        let directory =
            PrivateDirectory::open_beneath(&self.root, &relative).map_err(read_error)?;
        let file = directory
            .open_file(OsStr::new(encoded_identity), OpenMode::ReadNonblocking)
            .map_err(read_error)?;
        Ok(Box::new(BoundRecord {
            directory,
            file,
            name: encoded_identity.to_owned(),
        }))
    }

    fn visit_records(
        &self,
        visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
    ) -> Result<(), VerificationFailure> {
        let mut manifest = false;
        let mut records = false;
        for entry in fs::read_dir(&self.root).map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            let kind = entry.file_type().map_err(read_error)?;
            match entry.file_name().to_str() {
                Some("manifest.txt") if kind.is_file() => manifest = true,
                Some("records") if kind.is_dir() => records = true,
                Some("manifest.txt" | "records") => return Err(VerificationFailure::UnsafeObject),
                _ => return Err(VerificationFailure::InvalidLayout),
            }
        }
        if !manifest {
            return Err(VerificationFailure::Unreadable);
        }
        if !records {
            return Ok(());
        }
        let directory =
            PrivateDirectory::open_beneath(&self.root, Path::new("records")).map_err(read_error)?;
        for entry in directory.entries().map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            if entry.file_type() != PrivateFileType::Directory {
                return Err(VerificationFailure::UnsafeObject);
            }
            let store = entry
                .name()
                .to_str()
                .and_then(StoreKind::parse)
                .ok_or(VerificationFailure::InvalidLayout)?;
            let relative = Path::new("records").join(store.as_str());
            let body_directory =
                PrivateDirectory::open_beneath(&self.root, &relative).map_err(read_error)?;
            for body in body_directory.entries().map_err(read_error)? {
                let body = body.map_err(read_error)?;
                if body.file_type() != PrivateFileType::RegularFile {
                    return Err(VerificationFailure::UnsafeObject);
                }
                let name = body
                    .name()
                    .to_str()
                    .ok_or(VerificationFailure::InvalidLayout)?;
                visitor(store, name)?;
            }
            body_directory.verify_binding().map_err(read_error)?;
        }
        directory.verify_binding().map_err(read_error)
    }
}

struct BoundRecord {
    directory: PrivateDirectory,
    file: File,
    name: String,
}

impl Read for BoundRecord {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read(bytes)?;
        if count == 0
            && !self
                .directory
                .named_file_is(OsStr::new(&self.name), &self.file)?
        {
            return Err(io::Error::other("bundle record binding changed"));
        }
        Ok(count)
    }
}

fn read_error(error: io::Error) -> VerificationFailure {
    if is_unsafe_file(&error) {
        VerificationFailure::UnsafeObject
    } else {
        VerificationFailure::Unreadable
    }
}
