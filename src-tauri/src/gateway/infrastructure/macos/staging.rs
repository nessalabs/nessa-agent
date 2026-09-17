//! Immutable runtime installations. Copy and verify in a private sibling directory,
//! then publish once; old and already-published generations are never rewritten.
use super::{generation::random_generation, runtime_fingerprint};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsString},
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::{Error, ErrorKind, Read},
    os::fd::AsRawFd,
    os::unix::fs::{symlink, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

struct TemporaryRuntime(PathBuf);
impl Drop for TemporaryRuntime {
    fn drop(&mut self) {
        // This guard owns only the random directory created by this attempt.
        let _ = fs::remove_dir_all(&self.0);
    }
}

type CloneFile = fn(&Path, &Path) -> Result<bool, String>;

pub(super) fn stage_runtime(
    source: &Path,
    installations: &Path,
    expected: &str,
) -> Result<PathBuf, String> {
    stage_runtime_using(source, installations, expected, clone_file)
}

fn stage_runtime_using(
    source: &Path,
    installations: &Path,
    expected: &str,
    clone: CloneFile,
) -> Result<PathBuf, String> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("Invalid runtime staging fingerprint".into());
    }
    utf8(installations)?;
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    utf8(&source)?;
    nessa_local_storage::create_directory(installations).map_err(|error| error.to_string())?;
    nessa_local_storage::sync_directory(
        installations
            .parent()
            .ok_or("Missing runtime installation parent")?,
    )
    .map_err(|error| error.to_string())?;
    let published = installations.join(expected);
    match fs::symlink_metadata(&published) {
        Ok(_) => {
            validate_runtime(&published, expected)?;
            return Ok(published);
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let temporary_path = installations.join(format!(".staging-{}", random_generation()?));
    DirBuilder::new()
        .mode(0o700)
        .create(&temporary_path)
        .map_err(|error| error.to_string())?;
    let temporary = TemporaryRuntime(temporary_path);
    copy_directory(&source, &source, &temporary.0, clone)?;
    validate_runtime(&temporary.0, expected)?;
    publish(&temporary.0, &published)?;
    // After publication the old temporary path no longer exists; its guard cannot
    // remove the published directory even if directory synchronization fails.
    nessa_local_storage::sync_directory(installations).map_err(|error| error.to_string())?;
    Ok(published)
}
fn publish(temporary: &Path, published: &Path) -> Result<(), String> {
    let temporary = CString::new(utf8(temporary)?).map_err(|error| error.to_string())?;
    let published = CString::new(utf8(published)?).map_err(|error| error.to_string())?;
    // Unlike rename(), exclusive publication cannot replace an existing empty dir.
    if unsafe { libc::renamex_np(temporary.as_ptr(), published.as_ptr(), libc::RENAME_EXCL) } != 0 {
        return Err(Error::last_os_error().to_string());
    }
    Ok(())
}
fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "Runtime paths must be valid UTF-8".into())
}
fn entry_name(name: OsString) -> Result<String, String> {
    name.into_string()
        .map_err(|_| "Runtime names must be valid UTF-8".into())
}
fn children(directory: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry_name(entry.file_name())?;
        entries.push((name, entry.path()));
    }
    // JavaScript Array.sort compares UTF-16 code units, not UTF-8 bytes.
    entries.sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
    Ok(entries)
}
fn checked_link(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let target = fs::read_link(path).map_err(|error| error.to_string())?;
    utf8(&target)?;
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if target.is_absolute() || !resolved.starts_with(root) {
        return Err("Runtime symlink must remain inside its installation".into());
    }
    Ok(target)
}
fn copy_directory(
    root: &Path,
    source: &Path,
    destination: &Path,
    clone: CloneFile,
) -> Result<(), String> {
    for (_, path) in children(source)? {
        let name = path.file_name().ok_or("Missing runtime entry name")?;
        let output = destination.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            symlink(checked_link(root, &path)?, &output).map_err(|error| error.to_string())?;
        } else if metadata.is_dir() {
            DirBuilder::new()
                .mode(0o700)
                .create(&output)
                .map_err(|error| error.to_string())?;
            copy_directory(root, &path, &output, clone)?;
        } else if metadata.is_file() {
            let mut input = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .map_err(|error| error.to_string())?;
            let opened = input.metadata().map_err(|error| error.to_string())?;
            if !opened.is_file() {
                return Err("Runtime changed to a non-file during copy".into());
            }
            let cloned = clone(&path, &output)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(!cloned)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .mode(0o600)
                .open(&output)
                .map_err(|error| error.to_string())?;
            if !cloned {
                std::io::copy(&mut input, &mut file).map_err(|error| error.to_string())?;
            }
            finalize_file(&file, opened.permissions().mode())?;
        } else {
            return Err("Unsupported runtime entry type".into());
        }
    }
    nessa_local_storage::sync_directory(destination).map_err(|error| error.to_string())
}

fn finalize_file(file: &fs::File, source_mode: u32) -> Result<(), String> {
    file.set_permissions(Permissions::from_mode(0o600 | (source_mode & 0o111)))
        .map_err(|error| error.to_string())?;
    normalize_extended_attributes(file)?;
    // copyfile clones still create a new directory entry whose file state must
    // reach disk before the containing directory can be published durably.
    file.sync_all().map_err(|error| error.to_string())
}

fn normalize_extended_attributes(file: &fs::File) -> Result<(), String> {
    const PROVENANCE: &[u8] = b"com.apple.provenance";
    let names = extended_attribute_names(file.as_raw_fd())?;
    for name in names {
        let attribute = CString::new(name.as_slice()).map_err(|error| error.to_string())?;
        if unsafe { libc::fremovexattr(file.as_raw_fd(), attribute.as_ptr(), 0) } == 0 {
            continue;
        }
        let error = Error::last_os_error();
        if name == PROVENANCE && error.raw_os_error() == Some(libc::EPERM) {
            // macOS attaches this protected process-provenance marker to files
            // created by both copy paths. It cannot carry bundle-supplied policy.
            continue;
        }
        if error.raw_os_error() != Some(libc::ENOATTR) {
            return Err(error.to_string());
        }
    }
    let remaining = extended_attribute_names(file.as_raw_fd())?;
    if remaining.iter().any(|name| name != PROVENANCE) {
        return Err("Staged runtime file retains extended attributes".into());
    }
    Ok(())
}

fn extended_attribute_names(file: libc::c_int) -> Result<Vec<Vec<u8>>, String> {
    let length = unsafe { libc::flistxattr(file, std::ptr::null_mut(), 0, 0) };
    if length < 0 {
        return Err(Error::last_os_error().to_string());
    }
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut bytes = vec![0u8; length as usize];
    let written = unsafe { libc::flistxattr(file, bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if written < 0 {
        return Err(Error::last_os_error().to_string());
    }
    bytes.truncate(written as usize);
    if bytes.last() != Some(&0) {
        return Err("Invalid extended attribute list".into());
    }
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn clone_file(source: &Path, destination: &Path) -> Result<bool, String> {
    let source = CString::new(utf8(source)?).map_err(|error| error.to_string())?;
    let destination = CString::new(utf8(destination)?).map_err(|error| error.to_string())?;
    let flags = libc::COPYFILE_DATA | libc::COPYFILE_CLONE_FORCE | libc::COPYFILE_NOFOLLOW_SRC;
    if unsafe {
        libc::copyfile(
            source.as_ptr(),
            destination.as_ptr(),
            std::ptr::null_mut(),
            flags,
        )
    } == 0
    {
        return Ok(true);
    }
    let error = Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV)
    ) {
        Ok(false)
    } else {
        Err(error.to_string())
    }
}
pub(super) fn launch_settings(runtime: &Path) -> (Value, String) {
    (
        json!([
            runtime.join("nessa"),
            "server",
            "--desktop-runtime",
            runtime
        ]),
        format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", runtime.display()),
    )
}
fn validate_private_tree(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Published runtime has another owner".into());
    }
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            return Err("Published runtime directories must remain private".into());
        }
        for (_, child) in children(path)? {
            validate_private_tree(&child)?;
        }
    } else if metadata.is_file() {
        if metadata.permissions().mode() & 0o7666 != 0o600 || metadata.nlink() != 1 {
            return Err("Published runtime files must remain private and unaliased".into());
        }
    } else {
        return Err("Unsupported published runtime entry".into());
    }
    Ok(())
}
fn validate_runtime(directory: &Path, expected: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Published runtime must be a real directory".into());
    }
    let manifest =
        fs::symlink_metadata(directory.join("manifest.json")).map_err(|error| error.to_string())?;
    if !manifest.is_file() || manifest.file_type().is_symlink() {
        return Err("Runtime manifest must be a regular file".into());
    }
    validate_private_tree(directory)?;
    if runtime_fingerprint(directory)? != expected || tree_fingerprint(directory)? != expected {
        return Err("Runtime content does not match its manifest; installation preserved".into());
    }
    Ok(())
}
fn tree_fingerprint(directory: &Path) -> Result<String, String> {
    let root = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    hash_entry(&root, &root, "", &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn frame(hash: &mut Sha256, value: Value) -> Result<(), String> {
    hash.update(serde_json::to_vec(&value).map_err(|error| error.to_string())?);
    hash.update(b"\n");
    Ok(())
}
fn hash_entry(root: &Path, path: &Path, name: &str, hash: &mut Sha256) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        let target = checked_link(root, path)?;
        frame(hash, json!([name, "link", utf8(&target)?]))?;
    } else if metadata.is_dir() {
        frame(hash, json!([name, "directory"]))?;
        for (child, path) in children(path)? {
            if name.is_empty() && child == "manifest.json" {
                continue;
            }
            let relative = if name.is_empty() {
                child
            } else {
                format!("{name}/{child}")
            };
            hash_entry(root, &path, &relative, hash)?;
        }
    } else if metadata.is_file() {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Runtime changed during fingerprinting".into());
        }
        frame(
            hash,
            json!([
                name,
                "file",
                metadata.permissions().mode() & 0o111,
                metadata.len()
            ]),
        )?;
        let mut bytes = [0u8; 65536];
        let mut count = 0u64;
        loop {
            let read = file.read(&mut bytes).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            count += read as u64;
            hash.update(&bytes[..read]);
        }
        if count != metadata.len() {
            return Err("Runtime file changed while hashing".into());
        }
    } else {
        return Err("Unsupported runtime entry type".into());
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/staging.rs"]
mod tests;
