use super::*;
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
};
pub fn open(path: &Path, mode: OpenMode) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(!matches!(mode, OpenMode::Read))
        .create(matches!(mode, OpenMode::OpenOrCreate))
        .create_new(matches!(mode, OpenMode::CreateNew))
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    verify_file(&file)?;
    Ok(file)
}
pub fn verify_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(unsafe_file());
    }
    Ok(())
}
pub fn verify_directory(path: &Path) -> io::Result<()> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)?;
    let metadata = directory.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(unsafe_file());
    }
    directory.set_permissions(fs::Permissions::from_mode(0o700))
}
pub fn create_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    verify_directory(path)
}
pub fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}
