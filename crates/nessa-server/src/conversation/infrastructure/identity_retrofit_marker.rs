//! The file that says the one-shot identity retrofit has finished on this
//! namespace. Temporary, with the retrofit.
//!
//! Its presence is the whole answer; it holds one line naming what it marks.
//! Written to a private temporary file, synced, published without replacing,
//! and its directory synced before it is acknowledged, so a marker that was
//! acknowledged survives a crash. Absent is "not done"; any other failure to
//! look is [`MarkerUnavailable`], which the retrofit treats as "run again".
use crate::conversation::application::identity_retrofit::{
    IdentityRetrofitMarker, MarkerFuture, MarkerUnavailable,
};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use std::{
    io::{ErrorKind, Write},
    path::PathBuf,
};

/// The marker's name in its directory.
pub const IDENTITY_RETROFIT_MARKER: &str = "391-fingerprint.done";

pub struct FileIdentityRetrofitMarker {
    directory: PathBuf,
}

impl FileIdentityRetrofitMarker {
    /// A marker kept in `directory`, which is created when it is first
    /// written, not before.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

impl IdentityRetrofitMarker for FileIdentityRetrofitMarker {
    fn done(&self) -> MarkerFuture<'_, bool> {
        let path = self.directory.join(IDENTITY_RETROFIT_MARKER);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || match open(&path, OpenMode::Read) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
                Err(_) => Err(MarkerUnavailable),
            })
            .await
            .map_err(|_| MarkerUnavailable)?
        })
    }

    fn mark_done(&self) -> MarkerFuture<'_, ()> {
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                create_directory(&directory).map_err(|_| MarkerUnavailable)?;
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|_| MarkerUnavailable)?;
                file.as_file_mut()
                    .write_all(b"mcp servers left the restoration identity (#391)\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| MarkerUnavailable)?;
                match file.publish(&directory.join(IDENTITY_RETROFIT_MARKER)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(MarkerUnavailable),
                }
                sync_directory(&directory).map_err(|_| MarkerUnavailable)
            })
            .await
            .map_err(|_| MarkerUnavailable)?
        })
    }
}
