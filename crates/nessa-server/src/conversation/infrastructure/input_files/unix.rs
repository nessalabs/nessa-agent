//! Unix workspace placement retains the selected root and publishes no-replace.
use crate::conversation::application::{EnvironmentFuture, HeldSourceFailure, LocalInputFile};
use nessa_local_storage::{OpenMode, PrivateDirectory};
use nessa_sdk::{
    application::agent_execution::providers::UserFileError,
    domain::common::value_objects::Sha256Digest,
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::atomic::Ordering,
};

pub(super) fn place(
    request: LocalInputFile,
) -> EnvironmentFuture<'static, Result<String, UserFileError>> {
    let runtime = tokio::runtime::Handle::current();
    Box::pin(async move {
        tokio::task::spawn_blocking(move || {
            let workspace = Path::new(&request.workspace);
            let directory = PrivateDirectory::open_shared_root(workspace);
            let directory = directory.map_err(|_| UserFileError::Unavailable)?;
            let name = request.file.destination_name();
            let path = workspace
                .join(&name)
                .to_str()
                .ok_or(UserFileError::Unavailable)?
                .to_owned();
            if existing(
                &directory,
                &name,
                request.file.digest(),
                request.file.size(),
            )? {
                return Ok(path);
            }
            nessa_sdk::domain::agent_execution::prompts::LinkedFile::new(path.clone())
                .map_err(|_| UserFileError::Mismatch)?;
            let mut temporary = directory
                .reserve_temp()
                .map_err(|_| UserFileError::Unavailable)?;
            let mut offset = 0;
            let mut hash = Sha256::new();
            while offset < request.file.size() {
                if request.cancelled.load(Ordering::SeqCst) {
                    return Err(UserFileError::Cancelled);
                }
                let bytes = runtime
                    .block_on(request.source.read(offset, 32 * 1024))
                    .map_err(|error| match error {
                        HeldSourceFailure::NotHeld => UserFileError::NotHeld,
                        HeldSourceFailure::Unavailable => UserFileError::Unavailable,
                    })?;
                if bytes.is_empty() || bytes.len() as u64 > request.file.size() - offset {
                    return Err(UserFileError::Mismatch);
                }
                temporary
                    .as_file_mut()
                    .write_all(&bytes)
                    .map_err(|_| UserFileError::Unavailable)?;
                offset += bytes.len() as u64;
                hash.update(&bytes);
            }
            if Sha256Digest::from_bytes(hash.finalize().into()) != request.file.digest() {
                return Err(UserFileError::Mismatch);
            }
            request.delivered.store(true, Ordering::SeqCst);
            if request.cancelled.load(Ordering::SeqCst) {
                return Err(UserFileError::Cancelled);
            }
            match temporary.publish_new(OsStr::new(&name)) {
                Ok(_) => Ok(path),
                Err(error) if error.published().is_some() => Err(UserFileError::Uncertain),
                Err(error) if error.source_error().kind() == std::io::ErrorKind::AlreadyExists => {
                    if existing(
                        &directory,
                        &name,
                        request.file.digest(),
                        request.file.size(),
                    )? {
                        Ok(path)
                    } else {
                        Err(UserFileError::Occupied)
                    }
                }
                Err(_) => Err(UserFileError::Unavailable),
            }
        })
        .await
        .map_err(|_| UserFileError::Uncertain)?
    })
}
fn existing(
    directory: &PrivateDirectory,
    name: &str,
    digest: Sha256Digest,
    size: u64,
) -> Result<bool, UserFileError> {
    let mut file = match directory.open_file(OsStr::new(name), OpenMode::ReadNonblocking) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(UserFileError::Occupied),
    };
    if file
        .metadata()
        .map_err(|_| UserFileError::Unavailable)?
        .len()
        != size
    {
        return Err(UserFileError::Occupied);
    }
    let actual = digest_file(&mut file, size)?;
    if actual != digest {
        return Err(UserFileError::Occupied);
    }
    if !directory
        .named_file_is(OsStr::new(name), &file)
        .map_err(|_| UserFileError::Unavailable)?
    {
        return Err(UserFileError::Uncertain);
    }
    Ok(true)
}
fn digest_file(file: &mut File, size: u64) -> Result<Sha256Digest, UserFileError> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 32 * 1024];
    let mut reader = file.take(size + 1);
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| UserFileError::Unavailable)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(Sha256Digest::from_bytes(hash.finalize().into()))
}
