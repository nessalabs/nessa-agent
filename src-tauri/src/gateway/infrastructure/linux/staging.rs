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
    match fs::symlink_metadata(&path) {
        Ok(_) => remove_exact_tree(&path).map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
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

fn validate_runtime(directory: &Path, expected: &str) -> Result<(), String> {
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

fn random_hex() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn publish_bytes(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
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
    create_owned_directory_chain(parent)?;
    match read_owned_file(path) {
        Ok(Some(existing)) => {
            return (existing == bytes)
                .then_some(())
                .ok_or_else(|| "A conflicting owned definition occupies the planned path".into())
        }
        Ok(None) => {}
        Err(error) => return Err(error),
    }
    let parent_path = CString::new(parent.as_os_str().as_bytes())
        .map_err(|_| "Gateway definition directory contains NUL".to_string())?;
    let directory_descriptor = unsafe {
        libc::open(
            parent_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if directory_descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let directory = unsafe { File::from_raw_fd(directory_descriptor) };
    let temporary = CString::new(format!(".nessa-publish-{}", random_hex()?))
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
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    if renameat_noreplace(
        directory.as_raw_fd(),
        &temporary,
        directory.as_raw_fd(),
        &destination,
    ) != 0
    {
        unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) };
        return Err(std::io::Error::last_os_error().to_string());
    }
    directory.sync_all().map_err(|error| error.to_string())
}

pub(super) fn publish_wants_link(
    directory: &Path,
    link: &Path,
    unit_file: &Path,
) -> Result<(), String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };

    create_owned_directory_chain(directory)?;
    if link.parent() != Some(directory) || unit_file.parent() != directory.parent() {
        return Err("Persistent gateway link is outside its owned sibling directory".into());
    }
    let directory_path = CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| "Persistent gateway directory contains NUL".to_string())?;
    let descriptor = unsafe {
        libc::open(
            directory_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let directory_file = unsafe { File::from_raw_fd(descriptor) };
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
        return (observed == target_bytes)
            .then_some(())
            .ok_or_else(|| "A conflicting persistent gateway link already exists".into());
    }
    let temporary_name = CString::new(format!(".nessa-link-{}", random_hex()?))
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
    let renamed = renameat_noreplace(
        directory_file.as_raw_fd(),
        &temporary_name,
        directory_file.as_raw_fd(),
        &link_name,
    );
    if renamed != 0 {
        unsafe {
            libc::unlinkat(directory_file.as_raw_fd(), temporary_name.as_ptr(), 0);
        }
        return Err(std::io::Error::last_os_error().to_string());
    }
    directory_file
        .sync_all()
        .map_err(|error| error.to_string())?;
    let observed = read_link_at(directory_file.as_raw_fd(), &link_name)?
        .ok_or_else(|| "Published gateway link disappeared before acknowledgement".to_string())?;
    (observed == target_bytes)
        .then_some(())
        .ok_or_else(|| "Published gateway link changed before acknowledgement".into())
}

pub(super) fn wants_link_matches(link: &Path, unit_file: &Path) -> Result<bool, String> {
    let expected = Path::new("..").join(
        unit_file
            .file_name()
            .ok_or_else(|| "Gateway unit file has no name".to_string())?,
    );
    match fs::read_link(link) {
        Ok(observed) => Ok(observed == expected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn bytes_match(path: &Path, expected: &[u8]) -> Result<bool, String> {
    read_owned_file(path).map(|observed| observed.is_some_and(|value| value == expected))
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

pub(super) fn create_owned_directory_chain(path: &Path) -> Result<(), String> {
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
        if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.to_string());
            }
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
        if !metadata.is_dir() || (metadata.uid() != 0 && metadata.uid() != effective_uid) {
            return Err("Gateway directory ancestry has an unsafe owner or type".into());
        }
        directory = child;
    }
    if !saw_root {
        return Err("Gateway directory must be absolute".into());
    }
    let metadata = directory.metadata().map_err(|error| error.to_string())?;
    if metadata.uid() != effective_uid {
        return Err("Gateway directory is not owned by the effective account".into());
    }
    Ok(())
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

#[cfg(not(target_os = "linux"))]
fn renameat_noreplace(
    _from_directory: i32,
    _from: &std::ffi::CStr,
    _to_directory: i32,
    _to: &std::ffi::CStr,
) -> libc::c_long {
    -1
}

fn remove_exact_tree(path: &Path) -> Result<(), String> {
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
fn remove_exact_tree_linux(path: &Path) -> Result<(), String> {
    use std::{
        ffi::{CStr, CString},
        mem::MaybeUninit,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };

    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    let temporary_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".staging-"));
    if !temporary_name
        || !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("Temporary gateway runtime identity changed".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Temporary gateway runtime has no parent".to_string())?;
    let parent_path = CString::new(parent.as_os_str().as_bytes())
        .map_err(|_| "Temporary gateway runtime parent contains NUL".to_string())?;
    let parent_descriptor = unsafe {
        libc::open(
            parent_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if parent_descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let parent = unsafe { File::from_raw_fd(parent_descriptor) };
    let name = CString::new(
        path.file_name()
            .ok_or_else(|| "Temporary gateway runtime has no name".to_string())?
            .as_bytes(),
    )
    .map_err(|_| "Temporary gateway runtime name contains NUL".to_string())?;
    let child_descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if child_descriptor < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let child = unsafe { File::from_raw_fd(child_descriptor) };
    let opened = child.metadata().map_err(|error| error.to_string())?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err("Temporary gateway runtime changed before cleanup".into());
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
            let kind = stat.st_mode & libc::S_IFMT;
            if kind == libc::S_IFDIR {
                let descriptor = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if descriptor < 0 {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let child = unsafe { File::from_raw_fd(descriptor) };
                if let Err(error) = remove_children(&child, effective_uid) {
                    unsafe { libc::closedir(stream) };
                    return Err(error);
                }
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::last_os_error().to_string());
                }
            } else if kind == libc::S_IFREG || kind == libc::S_IFLNK {
                if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
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
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    parent.sync_all().map_err(|error| error.to_string())
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
        let directory = temporary.path().join("systemd/user");
        let definition = directory.join("nessa-gateway-prod.service");
        publish_bytes(&definition, b"first", 0o600).unwrap();
        assert_eq!(fs::read(&definition).unwrap(), b"first");
        assert!(publish_bytes(&definition, b"other", 0o600).is_err());

        let linked = directory.join("linked.service");
        std::os::unix::fs::symlink(&definition, &linked).unwrap();
        assert!(publish_bytes(&linked, b"first", 0o600).is_err());
        assert_eq!(fs::read(&definition).unwrap(), b"first");
    }

    #[test]
    fn wants_link_publication_keeps_the_first_exact_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let unit_root = temporary.path().join("systemd/user");
        let wants = unit_root.join("default.target.wants");
        let unit = unit_root.join("nessa-gateway-prod.service");
        publish_bytes(&unit, b"unit", 0o600).unwrap();
        let link = wants.join("nessa-gateway-prod.service");
        publish_wants_link(&wants, &link, &unit).unwrap();
        assert!(wants_link_matches(&link, &unit).unwrap());

        let other = unit_root.join("other.service");
        publish_bytes(&other, b"other", 0o600).unwrap();
        assert!(publish_wants_link(&wants, &link, &other).is_err());
        assert!(wants_link_matches(&link, &unit).unwrap());
    }
}
