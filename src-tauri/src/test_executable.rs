//! Putting a script a test is about to run on disk, without `ETXTBSY`.
//!
//! Linux refuses to `execve` a file that any process has open for writing:
//! `ETXTBSY`, "Text file busy". Writing a script with `fs::write` and then
//! running it looks safe, because the write's descriptor is closed before the
//! call returns. But a test binary runs many tests on many threads. While one
//! thread holds that descriptor, another thread's `Command::spawn` forks; the
//! child gets a copy of the whole descriptor table, and the copy stays open
//! until that child reaches its own `execve`, where close-on-exec drops it. For
//! that window the script is open for writing in a process nobody was thinking
//! about, and any test that runs its own script then fails to start it. Which
//! test fails depends on which forks happen to overlap, so a different one
//! fails on each run. It is the shape Go carries `ForkLock` for.
//!
//! Retrying the exec would make that rarer. [`install`] makes it unreachable:
//! the text is written to a sibling file that is never run, and `cp` puts the
//! runnable copy in place. Only `cp` ever opens that copy for writing, in its
//! own process, and its exit is waited for, so no fork of the test binary can
//! hold a writer on it, and nothing writes it again afterwards. The same window
//! over the text file means nothing, because the text file is never run.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

/// Writes `text` as an executable at `path`, owner-only, through `cp`.
pub(crate) fn install(path: &Path, text: &str) {
    let mut source = path.as_os_str().to_owned();
    source.push(".text");
    let source = PathBuf::from(source);
    fs::write(&source, text).unwrap();
    let copy = ["/bin/cp", "/usr/bin/cp"]
        .into_iter()
        .map(PathBuf::from)
        .find(|command| command.is_file())
        .expect("a Unix host has cp");
    let copied = Command::new(copy)
        .arg(&source)
        .arg(path)
        .status()
        .expect("cp runs");
    assert!(
        copied.success(),
        "the test executable could not be put in place"
    );
    // Changing the mode opens nothing for writing.
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
