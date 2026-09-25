use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, DirBuilder, File, OpenOptions, Permissions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct RuntimeManifest {
    fingerprint: String,
}

pub(super) fn runtime_fingerprint(runtime: &Path) -> Result<String, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(runtime.join("manifest.json"))
        .map_err(|_| "Missing or unsafe runtime manifest".to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Runtime manifest must be a regular file".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65_536 {
        return Err("Runtime manifest exceeds limit".into());
    }
    let manifest: RuntimeManifest =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid runtime manifest".to_string())?;
    validate_fingerprint(&manifest.fingerprint)?;
    Ok(manifest.fingerprint)
}

pub(super) fn publish_runtime(
    source: &Path,
    root: &Path,
    fingerprint: &str,
    staging_generation: &str,
) -> Result<PathBuf, String> {
    validate_fingerprint(fingerprint)?;
    validate_fingerprint(staging_generation)?;
    create_owned_directory_chain(root)?;
    let destination = root.join(fingerprint);
    if fs::symlink_metadata(&destination).is_ok() {
        validate_runtime(&destination, fingerprint)?;
        return Ok(destination);
    }
    let temporary = staging_runtime(root, staging_generation);
    DirBuilder::new()
        .mode(0o700)
        .create(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = copy_directory(source, source, &temporary)
        .and_then(|()| validate_runtime(&temporary, fingerprint))
        .and_then(|()| rename_noreplace(&temporary, &destination))
        .and_then(|()| sync_directory(root))
    {
        let _ = remove_exact_tree(&temporary);
        return Err(error);
    }
    validate_runtime(&destination, fingerprint)?;
    Ok(destination)
}

pub(super) fn staging_runtime(root: &Path, generation: &str) -> PathBuf {
    root.join(format!(".staging-{generation}"))
}

pub(super) fn remove_staging_runtime(root: &Path, generation: &str) -> Result<bool, String> {
    validate_fingerprint(generation)?;
    let path = staging_runtime(root, generation);
    remove_exact_tree(&path)
}

fn copy_directory(root: &Path, source: &Path, destination: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(source)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by(|left, right| {
        left.file_name()
            .to_string_lossy()
            .encode_utf16()
            .cmp(right.file_name().to_string_lossy().encode_utf16())
    });
    for entry in entries {
        let path = entry.path();
        let output = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            let target = checked_link(root, &path)?;
            std::os::unix::fs::symlink(target, &output).map_err(|error| error.to_string())?;
        } else if metadata.is_dir() {
            DirBuilder::new()
                .mode(0o700)
                .create(&output)
                .map_err(|error| error.to_string())?;
            copy_directory(root, &path, &output)?;
            sync_directory(&output)?;
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
            let mut output_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&output)
                .map_err(|error| error.to_string())?;
            std::io::copy(&mut input, &mut output_file).map_err(|error| error.to_string())?;
            output_file
                .set_permissions(Permissions::from_mode(
                    0o600 | (opened.permissions().mode() & 0o111),
                ))
                .map_err(|error| error.to_string())?;
            output_file.sync_all().map_err(|error| error.to_string())?;
        } else {
            return Err("Unsupported runtime entry type".into());
        }
    }
    sync_directory(destination)
}

fn checked_link(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let target = fs::read_link(path).map_err(|error| error.to_string())?;
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if target.is_absolute()
        || !resolved.starts_with(root.canonicalize().map_err(|error| error.to_string())?)
    {
        return Err("Runtime symlink must remain inside its installation".into());
    }
    Ok(target)
}

pub(super) fn validate_runtime(directory: &Path, expected: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Published runtime must be a real directory".into());
    }
    validate_private_tree(directory)?;
    if runtime_fingerprint(directory)? != expected || tree_fingerprint(directory)? != expected {
        return Err("Runtime content does not match its manifest; installation preserved".into());
    }
    Ok(())
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
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            validate_private_tree(&entry.map_err(|error| error.to_string())?.path())?;
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

fn tree_fingerprint(directory: &Path) -> Result<String, String> {
    let root = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    hash_entry(&root, &root, "", &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn hash_entry(root: &Path, path: &Path, name: &str, hash: &mut Sha256) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        let target = checked_link(root, path)?;
        frame(hash, json!([name, "link", target.to_string_lossy()]))?;
    } else if metadata.is_dir() {
        frame(hash, json!([name, "directory"]))?;
        let mut entries = fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by(|left, right| {
            left.file_name()
                .to_string_lossy()
                .encode_utf16()
                .cmp(right.file_name().to_string_lossy().encode_utf16())
        });
        for entry in entries {
            let child = entry
                .file_name()
                .into_string()
                .map_err(|_| "Runtime names must be UTF-8")?;
            if name.is_empty() && child == "manifest.json" {
                continue;
            }
            let relative = if name.is_empty() {
                child
            } else {
                format!("{name}/{child}")
            };
            hash_entry(root, &entry.path(), &relative, hash)?;
        }
    } else if metadata.is_file() {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| error.to_string())?;
        frame(
            hash,
            json!([
                name,
                "file",
                metadata.permissions().mode() & 0o111,
                metadata.len()
            ]),
        )?;
        let mut bytes = [0_u8; 65_536];
        loop {
            let read = file.read(&mut bytes).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            hash.update(&bytes[..read]);
        }
    } else {
        return Err("Unsupported runtime entry type".into());
    }
    Ok(())
}

fn frame(hash: &mut Sha256, value: Value) -> Result<(), String> {
    hash.update(serde_json::to_vec(&value).map_err(|error| error.to_string())?);
    hash.update(b"\n");
    Ok(())
}

fn validate_fingerprint(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err("Invalid runtime fingerprint".into())
    }
}

pub(super) fn publish_bytes(
    path: &Path,
    bytes: &[u8],
    mode: u32,
    transaction: &str,
) -> Result<(), String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };

    let parent = path
        .parent()
        .ok_or_else(|| "Published file has no parent".to_string())?;
    let directory = open_owned_directory_chain(parent)?;
    match read_owned_file(path) {
        Ok(Some(existing)) => {
            return (existing == bytes)
                .then_some(())
                .ok_or_else(|| "A conflicting owned definition occupies the planned path".into())
        }
        Ok(None) => {}
        Err(error) => return Err(error),
    }
    validate_fingerprint(transaction)?;
    let temporary = CString::new(format!(".nessa-publish-{transaction}"))
        .map_err(|_| "Temporary gateway definition name contains NUL".to_string())?;
    let destination = CString::new(
        path.file_name()
            .ok_or_else(|| "Gateway definition has no file name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Gateway definition name contains NUL".to_string())?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut file = unsafe { File::from_raw_fd(descriptor) };
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = unlink_named_file_if(&directory, &temporary, &file);
        let _ = directory.sync_all();
        return Err(error.to_string());
    }
    if renameat_noreplace(
        directory.as_raw_fd(),
        &temporary,
        directory.as_raw_fd(),
        &destination,
    ) != 0
    {
        let error = std::io::Error::last_os_error();
        let _ = unlink_named_file_if(&directory, &temporary, &file);
        let _ = directory.sync_all();
        return Err(error.to_string());
    }
    directory.sync_all().map_err(|error| error.to_string())
}

/// Replace the exact previously verified owned definition with the next one.
///
/// Linux exchanges the two names first, so the displaced definition remains
/// available for an identity check and rollback. A same-name substitution is
/// preserved and refused rather than overwritten or deleted.
pub(super) fn replace_owned_bytes(
    path: &Path,
    expected: &[u8],
    replacement: &[u8],
    generation: &str,
) -> Result<(), String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };

    validate_fingerprint(generation)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Published file has no parent".to_string())?;
    let directory = open_owned_directory_chain(parent)?;
    let destination = CString::new(
        path.file_name()
            .ok_or_else(|| "Gateway definition has no file name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Gateway definition name contains NUL".to_string())?;
    let prior = open_owned_file_at(&directory, &destination)?
        .ok_or_else(|| "The prior gateway definition disappeared before replacement".to_string())?;
    if prior.bytes != expected {
        return Err("The prior gateway definition changed before replacement".into());
    }
    let temporary = CString::new(format!(".nessa-replace-{generation}"))
        .map_err(|_| "Replacement name contains NUL".to_string())?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut new_file = unsafe { File::from_raw_fd(descriptor) };
    if let Err(error) = new_file
        .write_all(replacement)
        .and_then(|()| new_file.sync_all())
    {
        let _ = unlink_named_file_if(&directory, &temporary, &new_file);
        let _ = directory.sync_all();
        return Err(error.to_string());
    }
    if !named_file_is(&directory, &destination, &prior.file)? {
        let _ = unlink_named_file_if(&directory, &temporary, &new_file);
        let _ = directory.sync_all();
        return Err("The prior gateway definition changed before atomic replacement".into());
    }
    if renameat_exchange(
        directory.as_raw_fd(),
        &temporary,
        directory.as_raw_fd(),
        &destination,
    ) != 0
    {
        let error = std::io::Error::last_os_error();
        let _ = unlink_named_file_if(&directory, &temporary, &new_file);
        let _ = directory.sync_all();
        return Err(error.to_string());
    }
    let new_is_current = named_file_is(&directory, &destination, &new_file)?;
    let displaced_is_prior = named_file_is(&directory, &temporary, &prior.file)?;
    if !new_is_current || !displaced_is_prior {
        return Err("The prior gateway definition was substituted during replacement".into());
    }
    unlink_named_file_if(&directory, &temporary, &prior.file)?;
    directory.sync_all().map_err(|error| error.to_string())?;
    if !named_file_is(&directory, &destination, &new_file)? {
        return Err("The replacement gateway definition changed before acknowledgement".into());
    }
    Ok(())
}

struct OpenedOwnedFile {
    file: File,
    bytes: Vec<u8>,
}

fn open_owned_file_at(
    directory: &File,
    name: &std::ffi::CStr,
) -> Result<Option<OpenedOwnedFile>, String> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error.to_string());
    }
    let mut file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err("Gateway definition identity is unsafe".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(Some(OpenedOwnedFile { file, bytes }))
}

fn read_owned_file_at(directory: &File, name: &std::ffi::CStr) -> Result<Option<Vec<u8>>, String> {
    open_owned_file_at(directory, name).map(|opened| opened.map(|opened| opened.bytes))
}

fn unlink_named_file_if(
    directory: &File,
    name: &std::ffi::CStr,
    expected: &File,
) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    if !named_file_is(directory, name, expected)? {
        return Err("Gateway temporary file changed before cleanup".into());
    }
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn named_file_is(directory: &File, name: &std::ffi::CStr, expected: &File) -> Result<bool, String> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Ok(false);
    }
    let observed = unsafe { File::from_raw_fd(descriptor) };
    let observed = observed.metadata().map_err(|error| error.to_string())?;
    let expected = expected.metadata().map_err(|error| error.to_string())?;
    Ok(observed.dev() == expected.dev() && observed.ino() == expected.ino())
}

#[cfg(target_os = "linux")]
fn renameat_exchange(
    from_directory: i32,
    from: &std::ffi::CStr,
    to_directory: i32,
    to: &std::ffi::CStr,
) -> libc::c_long {
    unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from_directory,
            from.as_ptr(),
            to_directory,
            to.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    }
}

#[cfg(not(target_os = "linux"))]
fn renameat_exchange(
    _from_directory: i32,
    _from: &std::ffi::CStr,
    _to_directory: i32,
    _to: &std::ffi::CStr,
) -> libc::c_long {
    -1
}

pub(super) fn publish_wants_link(
    directory: &Path,
    link: &Path,
    unit_file: &Path,
    transaction: &str,
) -> Result<(), String> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };

    let directory_file = open_owned_directory_chain(directory)?;
    if link.parent() != Some(directory) || unit_file.parent() != directory.parent() {
        return Err("Persistent gateway link is outside its owned sibling directory".into());
    }
    let link_name = CString::new(
        link.file_name()
            .ok_or_else(|| "Persistent gateway link has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Persistent gateway link name contains NUL".to_string())?;
    let unit_name = unit_file
        .file_name()
        .ok_or_else(|| "Gateway unit file has no name".to_string())?;
    let target = Path::new("..").join(unit_name);
    let target_bytes = target.as_os_str().as_bytes();
    if let Some(observed) = read_link_at(directory_file.as_raw_fd(), &link_name)? {
        let identity = entry_identity_at(&directory_file, &link_name)?
            .ok_or_else(|| "Persistent gateway link disappeared during verification".to_string())?;
        return (observed == target_bytes
            && identity.kind == libc::S_IFLNK
            && identity.uid == unsafe { libc::geteuid() })
        .then_some(())
        .ok_or_else(|| "A conflicting or foreign persistent gateway link already exists".into());
    }
    validate_fingerprint(transaction)?;
    let temporary_name = CString::new(format!(".nessa-link-{transaction}"))
        .map_err(|_| "Temporary gateway link name contains NUL".to_string())?;
    let target_name = CString::new(target_bytes)
        .map_err(|_| "Persistent gateway link target contains NUL".to_string())?;
    if unsafe {
        libc::symlinkat(
            target_name.as_ptr(),
            directory_file.as_raw_fd(),
            temporary_name.as_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let temporary_identity = entry_identity_at(&directory_file, &temporary_name)?
        .ok_or_else(|| "Temporary gateway link disappeared after creation".to_string())?;
    let renamed = renameat_noreplace(
        directory_file.as_raw_fd(),
        &temporary_name,
        directory_file.as_raw_fd(),
        &link_name,
    );
    if renamed != 0 {
        let _ = unlink_link_if(&directory_file, &temporary_name, temporary_identity);
        return Err(std::io::Error::last_os_error().to_string());
    }
    directory_file
        .sync_all()
        .map_err(|error| error.to_string())?;
    let observed = read_link_at(directory_file.as_raw_fd(), &link_name)?
        .ok_or_else(|| "Published gateway link disappeared before acknowledgement".to_string())?;
    let published_identity = entry_identity_at(&directory_file, &link_name)?
        .ok_or_else(|| "Published gateway link disappeared before acknowledgement".to_string())?;
    (observed == target_bytes && published_identity == temporary_identity)
        .then_some(())
        .ok_or_else(|| "Published gateway link changed before acknowledgement".into())
}

pub(super) fn wants_link_matches(link: &Path, unit_file: &Path) -> Result<bool, String> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };

    let expected = Path::new("..").join(
        unit_file
            .file_name()
            .ok_or_else(|| "Gateway unit file has no name".to_string())?,
    );
    let parent = link
        .parent()
        .ok_or_else(|| "Persistent gateway link has no parent".to_string())?;
    if !parent.try_exists().map_err(|error| error.to_string())? {
        return Ok(false);
    }
    let directory = open_owned_directory_chain_existing(parent)?;
    let name = CString::new(
        link.file_name()
            .ok_or_else(|| "Persistent gateway link has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Persistent gateway link name contains NUL".to_string())?;
    let observed = read_link_at(directory.as_raw_fd(), &name)?;
    let identity = entry_identity_at(&directory, &name)?;
    Ok(observed.as_deref() == Some(expected.as_os_str().as_bytes())
        && identity.is_some_and(|identity| {
            identity.kind == libc::S_IFLNK && identity.uid == unsafe { libc::geteuid() }
        }))
}

pub(super) fn settle_wants_link_transaction(
    directory: &Path,
    link: &Path,
    unit_file: &Path,
    transaction: &str,
) -> Result<bool, String> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };

    validate_fingerprint(transaction)?;
    if !directory.try_exists().map_err(|error| error.to_string())? {
        return Ok(false);
    }
    if link.parent() != Some(directory) || unit_file.parent() != directory.parent() {
        return Err("Persistent gateway link is outside its owned sibling directory".into());
    }
    let parent = open_owned_directory_chain_existing(directory)?;
    let link_name = CString::new(
        link.file_name()
            .ok_or_else(|| "Persistent gateway link has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Persistent gateway link name contains NUL".to_string())?;
    let temporary = CString::new(format!(".nessa-link-{transaction}"))
        .map_err(|_| "Temporary gateway link name contains NUL".to_string())?;
    let target = Path::new("..").join(
        unit_file
            .file_name()
            .ok_or_else(|| "Gateway unit file has no name".to_string())?,
    );
    let expected = target.as_os_str().as_bytes();
    let current = read_link_at(parent.as_raw_fd(), &link_name)?;
    let pending = read_link_at(parent.as_raw_fd(), &temporary)?;
    if current.as_deref().is_some_and(|value| value != expected)
        || pending.as_deref().is_some_and(|value| value != expected)
    {
        return Err("Persistent gateway link transaction has contradictory content".into());
    }
    let pending_identity = entry_identity_at(&parent, &temporary)?;
    match (current, pending, pending_identity) {
        (None, None, None) => Ok(false),
        (Some(_), None, None) => wants_link_matches(link, unit_file),
        (None, Some(_), Some(identity)) => {
            if identity.kind != libc::S_IFLNK || identity.uid != unsafe { libc::geteuid() } {
                return Err("Temporary gateway link has unsafe identity".into());
            }
            if renameat_noreplace(
                parent.as_raw_fd(),
                &temporary,
                parent.as_raw_fd(),
                &link_name,
            ) != 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            parent.sync_all().map_err(|error| error.to_string())?;
            (entry_identity_at(&parent, &link_name)? == Some(identity))
                .then_some(true)
                .ok_or_else(|| "Recovered gateway link changed during publication".into())
        }
        (Some(_), Some(_), Some(identity)) => {
            if !wants_link_matches(link, unit_file)? {
                return Err("Published gateway link changed during recovery".into());
            }
            unlink_link_if(&parent, &temporary, identity)?;
            parent.sync_all().map_err(|error| error.to_string())?;
            Ok(true)
        }
        _ => Err("Persistent gateway link transaction changed during recovery".into()),
    }
}

pub(super) fn bytes_match(path: &Path, expected: &[u8]) -> Result<bool, String> {
    read_owned_file(path).map(|observed| observed.is_some_and(|value| value == expected))
}

pub(super) fn owned_file_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    read_owned_file(path)
}

pub(super) struct DefinitionTransaction {
    pub current: Option<Vec<u8>>,
    pub publish_temporary: Option<Vec<u8>>,
    pub replace_temporary: Option<Vec<u8>>,
}

pub(super) fn definition_transaction(
    path: &Path,
    generation: &str,
) -> Result<DefinitionTransaction, String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    validate_fingerprint(generation)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Gateway definition has no parent".to_string())?;
    if !parent.try_exists().map_err(|error| error.to_string())? {
        return Ok(DefinitionTransaction {
            current: None,
            publish_temporary: None,
            replace_temporary: None,
        });
    }
    let directory = open_owned_directory_chain_existing(parent)?;
    let current = CString::new(
        path.file_name()
            .ok_or_else(|| "Gateway definition has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Gateway definition name contains NUL".to_string())?;
    let publish = CString::new(format!(".nessa-publish-{generation}"))
        .map_err(|_| "Temporary publish name contains NUL".to_string())?;
    let replace = CString::new(format!(".nessa-replace-{generation}"))
        .map_err(|_| "Temporary replacement name contains NUL".to_string())?;
    Ok(DefinitionTransaction {
        current: read_owned_file_at(&directory, &current)?,
        publish_temporary: read_owned_file_at(&directory, &publish)?,
        replace_temporary: read_owned_file_at(&directory, &replace)?,
    })
}

pub(super) fn settle_definition_transaction(
    path: &Path,
    expected: &[u8],
    generation: &str,
) -> Result<bool, String> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };

    let transaction = definition_transaction(path, generation)?;
    if transaction.publish_temporary.is_some() && transaction.replace_temporary.is_some() {
        return Err("Both gateway definition temporary names are occupied".into());
    }
    let temporary = match (
        transaction.publish_temporary.as_deref(),
        transaction.replace_temporary.as_deref(),
    ) {
        (None, None) => return Ok(transaction.current.as_deref() == Some(expected)),
        (Some(publish), None) if publish == expected => format!(".nessa-publish-{generation}"),
        (None, Some(replace))
            if replace == expected || transaction.current.as_deref() == Some(expected) =>
        {
            format!(".nessa-replace-{generation}")
        }
        _ => return Err("A gateway definition temporary has contradictory content".into()),
    };
    if transaction.publish_temporary.is_some()
        && transaction
            .current
            .as_deref()
            .is_some_and(|current| current != expected)
    {
        return Err("A publish temporary conflicts with the installed definition".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Gateway definition has no parent".to_string())?;
    let directory = open_owned_directory_chain_existing(parent)?;
    let current_name = CString::new(
        path.file_name()
            .ok_or_else(|| "Gateway definition has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Gateway definition name contains NUL".to_string())?;
    let temporary = CString::new(temporary)
        .map_err(|_| "Gateway definition temporary name contains NUL".to_string())?;
    let current = open_owned_file_at(&directory, &current_name)?;
    let fresh = open_owned_file_at(&directory, &temporary)?
        .ok_or_else(|| "Gateway definition temporary changed before cleanup".to_string())?;
    let current_bytes = current.as_ref().map(|file| file.bytes.as_slice());
    let safe = current_bytes == transaction.current.as_deref()
        && (fresh.bytes == expected
            || (current_bytes == Some(expected)
                && transaction.replace_temporary.as_deref() == Some(fresh.bytes.as_slice())));
    if !safe {
        return Err("Gateway definition temporary changed before cleanup".into());
    }
    if let Some(current) = &current {
        if !named_file_is(&directory, &current_name, &current.file)? {
            return Err("Gateway definition changed before transaction cleanup".into());
        }
    }
    unlink_named_file_if(&directory, &temporary, &fresh.file)?;
    directory.sync_all().map_err(|error| error.to_string())?;
    Ok(current_bytes == Some(expected))
}

pub(super) fn staging_runtime_present(root: &Path, generation: &str) -> Result<bool, String> {
    validate_fingerprint(generation)?;
    let paths = [
        staging_runtime(root, generation),
        root.join(format!(".nessa-remove-{generation}")),
    ];
    let mut present = false;
    for path in paths {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err("Temporary gateway runtime identity changed".into());
        }
        if present {
            return Err("Both staged and quarantined runtimes occupy one transaction".into());
        }
        present = true;
    }
    Ok(present)
}

fn read_owned_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err("Gateway definition identity is unsafe".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err("Gateway definition changed during verification".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(Some(bytes))
}

fn read_link_at(
    directory: std::os::fd::RawFd,
    name: &std::ffi::CStr,
) -> Result<Option<Vec<u8>>, String> {
    let mut buffer = vec![0_u8; 4096];
    let length = unsafe {
        libc::readlinkat(
            directory,
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if length < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(error.to_string());
    }
    buffer.truncate(length as usize);
    Ok(Some(buffer))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EntryIdentity {
    device: libc::dev_t,
    inode: libc::ino_t,
    kind: libc::mode_t,
    uid: libc::uid_t,
}

fn entry_identity_at(
    directory: &File,
    name: &std::ffi::CStr,
) -> Result<Option<EntryIdentity>, String> {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};

    let mut stat = MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error.to_string());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(Some(EntryIdentity {
        device: stat.st_dev,
        inode: stat.st_ino,
        kind: stat.st_mode & libc::S_IFMT,
        uid: stat.st_uid,
    }))
}

fn unlink_link_if(
    directory: &File,
    name: &std::ffi::CStr,
    expected: EntryIdentity,
) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    if entry_identity_at(directory, name)? != Some(expected) {
        return Err("Gateway link identity changed before cleanup".into());
    }
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

pub(super) fn create_owned_directory_chain(path: &Path) -> Result<(), String> {
    open_owned_directory_chain(path).map(|_| ())
}

fn open_owned_directory_chain(path: &Path) -> Result<File, String> {
    open_owned_directory_chain_inner(path, true)
}

fn open_owned_directory_chain_existing(path: &Path) -> Result<File, String> {
    open_owned_directory_chain_inner(path, false)
}

fn open_owned_directory_chain_inner(path: &Path, create: bool) -> Result<File, String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
        path::Component,
    };
    let root = CString::new("/").expect("static root has no NUL");
    let descriptor = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut directory = unsafe { File::from_raw_fd(descriptor) };
    let effective_uid = unsafe { libc::geteuid() };
    let mut saw_root = false;
    for component in path.components() {
        let Component::Normal(component) = component else {
            if matches!(component, Component::RootDir) && !saw_root {
                saw_root = true;
                continue;
            }
            return Err("Gateway directory must be normalized and absolute".into());
        };
        let name = CString::new(component.as_bytes())
            .map_err(|_| "Gateway directory contains NUL".to_string())?;
        let created = if create {
            let created =
                unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } == 0;
            if !created {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error.to_string());
                }
            }
            created
        } else {
            false
        };
        if created {
            directory.sync_all().map_err(|error| error.to_string())?;
        }
        let child = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if child < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let child = unsafe { File::from_raw_fd(child) };
        let metadata = child.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_dir()
            || (metadata.uid() != 0 && metadata.uid() != effective_uid)
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err("Gateway directory ancestry has an unsafe owner or type".into());
        }
        directory = child;
    }
    if !saw_root {
        return Err("Gateway directory must be absolute".into());
    }
    let metadata = directory.metadata().map_err(|error| error.to_string())?;
    if metadata.uid() != effective_uid || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err("Gateway directory is not private to the effective account".into());
    }
    directory.sync_all().map_err(|error| error.to_string())?;
    Ok(directory)
}

fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let parent = from
        .parent()
        .filter(|parent| Some(*parent) == to.parent())
        .ok_or_else(|| "No-replace publication must stay in one directory".to_string())?;
    let parent = CString::new(parent.as_os_str().as_bytes()).map_err(|_| "NUL path")?;
    let descriptor = unsafe {
        libc::open(
            parent.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let directory = unsafe { File::from_raw_fd(descriptor) };
    let from = CString::new(
        from.file_name()
            .ok_or_else(|| "Source has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "NUL path")?;
    let to = CString::new(
        to.file_name()
            .ok_or_else(|| "Destination has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "NUL path")?;
    let result = renameat_noreplace(directory.as_raw_fd(), &from, directory.as_raw_fd(), &to);
    (result == 0)
        .then_some(())
        .ok_or_else(|| std::io::Error::last_os_error().to_string())
}

#[cfg(target_os = "linux")]
fn renameat_noreplace(
    from_directory: i32,
    from: &std::ffi::CStr,
    to_directory: i32,
    to: &std::ffi::CStr,
) -> libc::c_long {
    unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from_directory,
            from.as_ptr(),
            to_directory,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    }
}

#[cfg(target_os = "macos")]
fn renameat_noreplace(
    from_directory: i32,
    from: &std::ffi::CStr,
    to_directory: i32,
    to: &std::ffi::CStr,
) -> libc::c_long {
    unsafe {
        libc::renameatx_np(
            from_directory,
            from.as_ptr(),
            to_directory,
            to.as_ptr(),
            libc::RENAME_EXCL,
        ) as libc::c_long
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn renameat_noreplace(
    _from_directory: i32,
    _from: &std::ffi::CStr,
    _to_directory: i32,
    _to: &std::ffi::CStr,
) -> libc::c_long {
    -1
}

fn remove_exact_tree(path: &Path) -> Result<bool, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Err("Exact runtime cleanup requires Linux directory handles".into())
    }
    #[cfg(target_os = "linux")]
    {
        remove_exact_tree_linux(path)
    }
}

#[cfg(target_os = "linux")]
fn remove_exact_tree_linux(path: &Path) -> Result<bool, String> {
    use std::{
        ffi::{CStr, CString},
        mem::MaybeUninit,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };

    let temporary_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".staging-"));
    if !temporary_name {
        return Err("Temporary gateway runtime identity changed".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Temporary gateway runtime has no parent".to_string())?;
    let parent = open_owned_directory_chain_existing(parent)?;
    let name = CString::new(
        path.file_name()
            .ok_or_else(|| "Temporary gateway runtime has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Temporary gateway runtime name contains NUL".to_string())?;
    let generation = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".staging-"))
        .ok_or_else(|| "Temporary gateway runtime has no generation".to_string())?;
    validate_fingerprint(generation)?;
    let quarantine = CString::new(format!(".nessa-remove-{generation}"))
        .map_err(|_| "Temporary cleanup name contains NUL".to_string())?;
    let source = if entry_kind_at(&parent, &quarantine)?.is_some() {
        if entry_kind_at(&parent, &name)?.is_some() {
            return Err("Both staged and quarantined runtimes occupy one transaction".into());
        }
        &quarantine
    } else if entry_kind_at(&parent, &name)?.is_some() {
        if renameat_noreplace(parent.as_raw_fd(), &name, parent.as_raw_fd(), &quarantine) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        parent.sync_all().map_err(|error| error.to_string())?;
        &quarantine
    } else {
        return Ok(false);
    };
    let source_identity = entry_identity_at(&parent, source)?
        .ok_or_else(|| "Temporary gateway runtime disappeared before cleanup".to_string())?;
    let child_descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            source.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if child_descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let child = unsafe { File::from_raw_fd(child_descriptor) };
    let opened = child.metadata().map_err(|error| error.to_string())?;
    if !opened.is_dir()
        || opened.uid() != unsafe { libc::geteuid() }
        || opened.dev() != source_identity.device
        || opened.ino() != source_identity.inode
    {
        return Err("Temporary gateway runtime identity changed".into());
    }

    fn remove_children(directory: &File, effective_uid: u32) -> Result<(), String> {
        let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
        if duplicate < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            unsafe { libc::close(duplicate) };
            return Err(std::io::Error::last_os_error().to_string());
        }
        loop {
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let name = CString::new(name.to_bytes()).expect("directory entry excludes NUL");
            let mut stat = MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    stat.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                unsafe { libc::closedir(stream) };
                return Err(std::io::Error::last_os_error().to_string());
            }
            let stat = unsafe { stat.assume_init() };
            if stat.st_uid != effective_uid {
                unsafe { libc::closedir(stream) };
                return Err("Temporary gateway runtime gained an entry with another owner".into());
            }
            let quarantine = if name.as_bytes() == b".nessa-cleanup-a" {
                CString::new(".nessa-cleanup-b").expect("static cleanup name")
            } else {
                CString::new(".nessa-cleanup-a").expect("static cleanup name")
            };
            if renameat_noreplace(
                directory.as_raw_fd(),
                &name,
                directory.as_raw_fd(),
                &quarantine,
            ) != 0
            {
                unsafe { libc::closedir(stream) };
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut moved = MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    directory.as_raw_fd(),
                    quarantine.as_ptr(),
                    moved.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                unsafe { libc::closedir(stream) };
                return Err(std::io::Error::last_os_error().to_string());
            }
            let moved = unsafe { moved.assume_init() };
            if moved.st_dev != stat.st_dev
                || moved.st_ino != stat.st_ino
                || moved.st_mode != stat.st_mode
                || moved.st_uid != stat.st_uid
            {
                let _ = renameat_noreplace(
                    directory.as_raw_fd(),
                    &quarantine,
                    directory.as_raw_fd(),
                    &name,
                );
                unsafe { libc::closedir(stream) };
                return Err("Temporary gateway runtime entry changed before cleanup".into());
            }
            let kind = stat.st_mode & libc::S_IFMT;
            if kind == libc::S_IFDIR {
                let descriptor = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        quarantine.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if descriptor < 0 {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let child = unsafe { File::from_raw_fd(descriptor) };
                let child_metadata = child.metadata().map_err(|error| error.to_string())?;
                if child_metadata.dev() != moved.st_dev || child_metadata.ino() != moved.st_ino {
                    unsafe { libc::closedir(stream) };
                    return Err("Temporary gateway directory changed before cleanup".into());
                }
                if let Err(error) = remove_children(&child, effective_uid) {
                    unsafe { libc::closedir(stream) };
                    return Err(error);
                }
                if entry_identity_at(directory, &quarantine)?
                    != Some(EntryIdentity {
                        device: moved.st_dev,
                        inode: moved.st_ino,
                        kind,
                        uid: moved.st_uid,
                    })
                {
                    unsafe { libc::closedir(stream) };
                    return Err("Temporary gateway directory changed before removal".into());
                }
                if unsafe {
                    libc::unlinkat(
                        directory.as_raw_fd(),
                        quarantine.as_ptr(),
                        libc::AT_REMOVEDIR,
                    )
                } != 0
                {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::last_os_error().to_string());
                }
            } else if kind == libc::S_IFREG || kind == libc::S_IFLNK {
                if entry_identity_at(directory, &quarantine)?
                    != Some(EntryIdentity {
                        device: moved.st_dev,
                        inode: moved.st_ino,
                        kind,
                        uid: moved.st_uid,
                    })
                {
                    unsafe { libc::closedir(stream) };
                    return Err("Temporary gateway entry changed before removal".into());
                }
                if unsafe { libc::unlinkat(directory.as_raw_fd(), quarantine.as_ptr(), 0) } != 0 {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::last_os_error().to_string());
                }
            } else {
                unsafe { libc::closedir(stream) };
                return Err("Temporary gateway runtime gained an unsupported entry".into());
            }
        }
        if unsafe { libc::closedir(stream) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    remove_children(&child, unsafe { libc::geteuid() })?;
    if entry_identity_at(&parent, source)? != Some(source_identity) {
        return Err("Temporary gateway runtime changed before final removal".into());
    }
    if unsafe { libc::unlinkat(parent.as_raw_fd(), source.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    parent.sync_all().map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(target_os = "linux")]
fn entry_kind_at(directory: &File, name: &std::ffi::CStr) -> Result<Option<libc::mode_t>, String> {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};

    let mut stat = MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error.to_string());
    }
    Ok(Some(unsafe { stat.assume_init() }.st_mode & libc::S_IFMT))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_publish_once_and_never_follow_an_existing_link() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("systemd/user");
        let definition = directory.join("nessa-gateway-prod.service");
        publish_bytes(&definition, b"first", 0o600, &"a".repeat(64)).unwrap();
        assert_eq!(fs::read(&definition).unwrap(), b"first");
        assert!(publish_bytes(&definition, b"other", 0o600, &"b".repeat(64)).is_err());

        let linked = directory.join("linked.service");
        std::os::unix::fs::symlink(&definition, &linked).unwrap();
        assert!(publish_bytes(&linked, b"first", 0o600, &"c".repeat(64)).is_err());
        assert_eq!(fs::read(&definition).unwrap(), b"first");
    }

    #[test]
    fn wants_link_publication_keeps_the_first_exact_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let unit_root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("systemd/user");
        let wants = unit_root.join("default.target.wants");
        let unit = unit_root.join("nessa-gateway-prod.service");
        publish_bytes(&unit, b"unit", 0o600, &"a".repeat(64)).unwrap();
        let link = wants.join("nessa-gateway-prod.service");
        publish_wants_link(&wants, &link, &unit, &"b".repeat(64)).unwrap();
        assert!(wants_link_matches(&link, &unit).unwrap());

        let other = unit_root.join("other.service");
        publish_bytes(&other, b"other", 0o600, &"c".repeat(64)).unwrap();
        assert!(publish_wants_link(&wants, &link, &other, &"d".repeat(64)).is_err());
        assert!(wants_link_matches(&link, &unit).unwrap());
    }

    #[test]
    fn wants_link_observation_does_not_create_the_missing_parent() {
        let temporary = tempfile::tempdir().unwrap();
        let unit_root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("systemd/user");
        let wants = unit_root.join("default.target.wants");
        let unit = unit_root.join("nessa-gateway-prod.service");
        let link = wants.join("nessa-gateway-prod.service");
        assert!(!wants_link_matches(&link, &unit).unwrap());
        assert!(!wants.exists());
    }

    #[test]
    fn wants_link_recovery_settles_every_deterministic_temporary_state() {
        let temporary = tempfile::tempdir().unwrap();
        let unit_root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("systemd/user");
        let wants = unit_root.join("default.target.wants");
        create_owned_directory_chain(&wants).unwrap();
        let unit = unit_root.join("nessa-gateway-prod.service");
        publish_bytes(&unit, b"unit", 0o600, &"a".repeat(64)).unwrap();
        let link = wants.join("nessa-gateway-prod.service");
        let generation = "b".repeat(64);
        let pending = wants.join(format!(".nessa-link-{generation}"));
        std::os::unix::fs::symlink("../nessa-gateway-prod.service", &pending).unwrap();

        assert!(settle_wants_link_transaction(&wants, &link, &unit, &generation).unwrap());
        assert!(wants_link_matches(&link, &unit).unwrap());
        assert!(!pending.exists());

        std::os::unix::fs::symlink("../nessa-gateway-prod.service", &pending).unwrap();
        assert!(settle_wants_link_transaction(&wants, &link, &unit, &generation).unwrap());
        assert!(wants_link_matches(&link, &unit).unwrap());
        assert!(!pending.exists());
    }

    #[test]
    fn retained_file_identity_rejects_an_equal_content_replacement() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::set_permissions(&root, Permissions::from_mode(0o700)).unwrap();
        let path = root.join("definition.service");
        fs::write(&path, b"same").unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
        let directory = open_owned_directory_chain_existing(&root).unwrap();
        let name = CString::new(path.file_name().unwrap().as_bytes()).unwrap();
        let retained = open_owned_file_at(&directory, &name).unwrap().unwrap();
        let replacement = root.join("replacement");
        fs::write(&replacement, b"same").unwrap();
        fs::set_permissions(&replacement, Permissions::from_mode(0o600)).unwrap();
        fs::rename(&replacement, &path).unwrap();

        assert_eq!(retained.bytes, b"same");
        assert!(!named_file_is(&directory, &name, &retained.file).unwrap());
    }

    #[test]
    fn definition_transaction_cleanup_retains_the_committed_side() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap().join("owned");
        create_owned_directory_chain(&root).unwrap();
        let definition = root.join("nessa-gateway-prod.service");
        let generation = "f".repeat(64);
        publish_bytes(&definition, b"new", 0o600, &"a".repeat(64)).unwrap();
        let displaced = root.join(format!(".nessa-replace-{generation}"));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&displaced)
            .unwrap()
            .write_all(b"old")
            .unwrap();

        assert!(settle_definition_transaction(&definition, b"new", &generation).unwrap());
        assert_eq!(fs::read(&definition).unwrap(), b"new");
        assert!(!displaced.exists());
    }

    #[test]
    fn definition_transaction_cleanup_discards_an_uncommitted_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap().join("owned");
        create_owned_directory_chain(&root).unwrap();
        let definition = root.join("nessa-gateway-prod.service");
        let generation = "9".repeat(64);
        publish_bytes(&definition, b"old", 0o600, &"a".repeat(64)).unwrap();
        let replacement = root.join(format!(".nessa-replace-{generation}"));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&replacement)
            .unwrap()
            .write_all(b"new")
            .unwrap();

        assert!(!settle_definition_transaction(&definition, b"new", &generation).unwrap());
        assert_eq!(fs::read(&definition).unwrap(), b"old");
        assert!(!replacement.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exact_owned_definition_can_be_replaced_after_verified_retirement() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let definition = root.join("nessa-gateway-prod.service");
        publish_bytes(&definition, b"old", 0o600, &"c".repeat(64)).unwrap();
        replace_owned_bytes(&definition, b"old", b"new", &"a".repeat(64)).unwrap();
        assert_eq!(fs::read(&definition).unwrap(), b"new");
        assert!(!root
            .join(format!(".nessa-replace-{}", "a".repeat(64)))
            .exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exact_owned_definition_replacement_preserves_a_substitute() {
        let temporary = tempfile::tempdir().unwrap();
        let definition = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("nessa-gateway-prod.service");
        publish_bytes(&definition, b"substitute", 0o600, &"d".repeat(64)).unwrap();
        assert!(replace_owned_bytes(&definition, b"old", b"new", &"b".repeat(64)).is_err());
        assert_eq!(fs::read(&definition).unwrap(), b"substitute");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn staging_cleanup_resumes_from_the_durable_quarantine_name() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap().join("runtimes");
        create_owned_directory_chain(&root).unwrap();
        let generation = "e".repeat(64);
        let staging = staging_runtime(&root, &generation);
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, Permissions::from_mode(0o700)).unwrap();
        fs::write(staging.join("payload"), b"owned").unwrap();
        let quarantine = root.join(format!(".nessa-remove-{generation}"));
        fs::rename(&staging, &quarantine).unwrap();

        assert!(staging_runtime_present(&root, &generation).unwrap());
        assert!(remove_staging_runtime(&root, &generation).unwrap());
        assert!(!quarantine.exists());
        assert!(!remove_staging_runtime(&root, &generation).unwrap());
    }
}
