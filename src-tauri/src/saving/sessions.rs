//! The saves the page has begun and not yet finished: a file written next to
//! the place the person chose, filled one bounded part at a time, and renamed
//! into place only once it is whole.
//!
//! Each save is a temporary file in the chosen folder, created new (never
//! opened over something already there), named by the save's token and
//! nothing the page chose. Its parts are appended in the order they arrive,
//! and the whole is held to [`LARGEST_SAVED_BYTES`] as it grows. Finishing
//! syncs it and renames it over the chosen path; giving up, failing, or
//! falling idle for longer than the host waits removes it. So the chosen path
//! only ever holds the whole file or what was there before, and a page that
//! walks away leaves nothing behind once the host has noticed.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::rule::{FileNotSaved, NotSavedReason, LARGEST_SAVED_BYTES};

/// How long a save may sit with nothing sent before the host lets it go.
pub const LONGEST_IDLE: Duration = Duration::from_secs(60);

struct Open {
    target: PathBuf,
    temporary: PathBuf,
    file: File,
    written: u64,
    touched: Instant,
}

/// Every save in flight, by token.
pub struct SaveSessions {
    open: Mutex<HashMap<String, Open>>,
    most: u64,
    idle: Duration,
}

impl Default for SaveSessions {
    fn default() -> Self {
        Self::new(LARGEST_SAVED_BYTES, LONGEST_IDLE)
    }
}

fn not_written(error: impl std::fmt::Display) -> FileNotSaved {
    FileNotSaved::new(NotSavedReason::NotWritten, error.to_string())
}

fn not_open() -> FileNotSaved {
    FileNotSaved::new(NotSavedReason::NotOpen, "no save is open under that token")
}

/// Where a save's bytes gather before they are whole: beside `target`, so the
/// rename that finishes it stays on one filesystem.
fn temporary_beside(target: &Path, token: &str) -> Result<PathBuf, FileNotSaved> {
    let folder = target
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .ok_or_else(|| not_written("the chosen path is in no folder"))?;
    Ok(folder.join(format!(".nessa-save-{token}.part")))
}

impl SaveSessions {
    /// A ledger that holds a file to `most` bytes and lets a save go after
    /// `idle` with nothing sent.
    pub fn new(most: u64, idle: Duration) -> Self {
        Self {
            open: Mutex::new(HashMap::new()),
            most,
            idle,
        }
    }

    fn ledger(&self) -> std::sync::MutexGuard<'_, HashMap<String, Open>> {
        // A panic while the ledger was held left a map, not a broken one.
        self.open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Begin a save to `target` under `token`: its temporary file is created
    /// new beside it.
    pub fn open(&self, target: PathBuf, token: &str, now: Instant) -> Result<(), FileNotSaved> {
        let temporary = temporary_beside(&target, token)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(not_written)?;
        self.ledger().insert(
            token.to_string(),
            Open {
                target,
                temporary,
                file,
                written: 0,
                touched: now,
            },
        );
        Ok(())
    }

    /// Append `bytes` to the save under `token`. A part that would take the
    /// file past the bound, or that cannot be written, ends the save and
    /// removes what it had written.
    pub fn part(&self, token: &str, bytes: &[u8], now: Instant) -> Result<(), FileNotSaved> {
        let mut ledger = self.ledger();
        let open = ledger.get_mut(token).ok_or_else(not_open)?;
        let written = open.written + bytes.len() as u64;
        let outcome = if written > self.most {
            Err(FileNotSaved::new(
                NotSavedReason::TooLarge,
                format!(
                    "{written} bytes is more than the {} a save takes",
                    self.most
                ),
            ))
        } else {
            open.file.write_all(bytes).map_err(not_written)
        };
        match outcome {
            Ok(()) => {
                open.written = written;
                open.touched = now;
                Ok(())
            }
            Err(refusal) => {
                if let Some(open) = ledger.remove(token) {
                    discard(open);
                }
                Err(refusal)
            }
        }
    }

    /// Finish the save under `token`: its bytes synced, then renamed over the
    /// chosen path. Nothing is left under the token either way.
    pub fn finish(&self, token: &str) -> Result<(), FileNotSaved> {
        let open = self.ledger().remove(token).ok_or_else(not_open)?;
        if let Err(error) = open.file.sync_all() {
            discard(open);
            return Err(not_written(error));
        }
        let Open {
            target, temporary, ..
        } = open;
        if let Err(error) = fs::rename(&temporary, &target) {
            let _ = fs::remove_file(&temporary);
            return Err(not_written(error));
        }
        // The rename is durable once its folder is; a folder that cannot be
        // synced still holds the whole file.
        #[cfg(unix)]
        if let Some(folder) = target.parent() {
            let _ = File::open(folder).and_then(|folder| folder.sync_all());
        }
        Ok(())
    }

    /// Give up the save under `token`, removing what it had written. Giving up
    /// one that is not open is nothing.
    pub fn abort(&self, token: &str) {
        if let Some(open) = self.ledger().remove(token) {
            discard(open);
        }
    }

    /// Let go of every save idle for longer than the host waits, removing what
    /// each had written. Whether `token` is still open after.
    pub fn sweep(&self, now: Instant, token: &str) -> bool {
        let mut ledger = self.ledger();
        let idle: Vec<String> = ledger
            .iter()
            .filter(|(_, open)| now.saturating_duration_since(open.touched) > self.idle)
            .map(|(token, _)| token.clone())
            .collect();
        for gone in idle {
            if let Some(open) = ledger.remove(&gone) {
                discard(open);
            }
        }
        ledger.contains_key(token)
    }
}

fn discard(open: Open) {
    drop(open.file);
    let _ = fs::remove_file(&open.temporary);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_the_parts_into_the_chosen_file_only_once_it_is_whole() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("chart.png");
        let sessions = SaveSessions::default();
        let now = Instant::now();
        sessions.open(target.clone(), "t1", now).unwrap();
        sessions.part("t1", b"\x89PNG ", now).unwrap();
        sessions.part("t1", b"bytes", now).unwrap();
        // Not there until finished: only the temporary file is.
        assert_eq!(files_in(folder.path()), [".nessa-save-t1.part"]);
        sessions.finish("t1").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"\x89PNG bytes");
        assert_eq!(files_in(folder.path()), ["chart.png"]);
        // Finished, it takes nothing more.
        let late = sessions.part("t1", b"more", now).unwrap_err();
        assert_eq!(late.reason, NotSavedReason::NotOpen);
        assert_eq!(fs::read(&target).unwrap(), b"\x89PNG bytes");
    }

    #[test]
    fn replaces_what_was_at_the_chosen_path_whole() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("notes.txt");
        fs::write(&target, b"old").unwrap();
        let sessions = SaveSessions::default();
        sessions.open(target.clone(), "t", Instant::now()).unwrap();
        // Until it finishes, the old file is untouched.
        sessions.part("t", b"new", Instant::now()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old");
        sessions.finish("t").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn giving_up_removes_what_was_written_and_takes_nothing_after() {
        let folder = tempfile::tempdir().unwrap();
        let sessions = SaveSessions::default();
        sessions
            .open(folder.path().join("a.txt"), "t", Instant::now())
            .unwrap();
        sessions.part("t", b"half", Instant::now()).unwrap();
        sessions.abort("t");
        assert!(files_in(folder.path()).is_empty());
        let late = sessions.part("t", b"rest", Instant::now()).unwrap_err();
        assert_eq!(late.reason, NotSavedReason::NotOpen);
        assert_eq!(
            sessions.finish("t").unwrap_err().reason,
            NotSavedReason::NotOpen
        );
        assert!(files_in(folder.path()).is_empty());
    }

    #[test]
    fn refuses_a_file_past_the_bound_and_removes_it() {
        let folder = tempfile::tempdir().unwrap();
        let sessions = SaveSessions::new(10, LONGEST_IDLE);
        let now = Instant::now();
        sessions
            .open(folder.path().join("a.bin"), "t", now)
            .unwrap();
        sessions.part("t", &[0; 6], now).unwrap();
        let refusal = sessions.part("t", &[0; 5], now).unwrap_err();
        assert_eq!(refusal.reason, NotSavedReason::TooLarge);
        assert!(files_in(folder.path()).is_empty());
        assert_eq!(
            sessions.finish("t").unwrap_err().reason,
            NotSavedReason::NotOpen
        );
    }

    #[test]
    fn refuses_more_than_the_largest_file_a_conversation_holds() {
        let folder = tempfile::tempdir().unwrap();
        let sessions = SaveSessions::default();
        let now = Instant::now();
        sessions
            .open(folder.path().join("big.bin"), "t", now)
            .unwrap();
        let part = vec![0u8; super::super::rule::LARGEST_PART_BYTES];
        let parts = LARGEST_SAVED_BYTES as usize / part.len();
        for _ in 0..parts {
            sessions.part("t", &part, now).unwrap();
        }
        // Exactly the bound is a file; one byte more is not.
        let refusal = sessions.part("t", &[0], now).unwrap_err();
        assert_eq!(refusal.reason, NotSavedReason::TooLarge);
        assert!(files_in(folder.path()).is_empty());
    }

    #[test]
    fn lets_go_of_a_save_left_idle_and_removes_its_file() {
        let folder = tempfile::tempdir().unwrap();
        let sessions = SaveSessions::default();
        let start = Instant::now();
        sessions
            .open(folder.path().join("a.txt"), "idle", start)
            .unwrap();
        sessions
            .open(folder.path().join("b.txt"), "busy", start)
            .unwrap();
        sessions.part("idle", b"half", start).unwrap();
        let later = start + LONGEST_IDLE + Duration::from_secs(1);
        // The busy one sent a part just before the sweep.
        sessions.part("busy", b"b", later).unwrap();
        assert!(!sessions.sweep(later, "idle"));
        assert!(sessions.sweep(later, "busy"));
        assert_eq!(files_in(folder.path()), [".nessa-save-busy.part"]);
        assert_eq!(
            sessions.part("idle", b"rest", later).unwrap_err().reason,
            NotSavedReason::NotOpen
        );
    }

    #[test]
    fn never_opens_over_a_file_already_there() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(".nessa-save-t.part"), b"someone's").unwrap();
        let sessions = SaveSessions::default();
        let refusal = sessions
            .open(folder.path().join("a.txt"), "t", Instant::now())
            .unwrap_err();
        assert_eq!(refusal.reason, NotSavedReason::NotWritten);
        assert_eq!(
            fs::read(folder.path().join(".nessa-save-t.part")).unwrap(),
            b"someone's"
        );
    }
}
