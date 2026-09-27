//! The window the person chooses files in.
//!
//! A port of its own, separate from the filesystem: the picker is a window the
//! person interacts with, it takes as long as they take, and its answer is a
//! choice rather than a fact. Folding it into [`super::files::ChosenFiles`]
//! would mean a substitute could only pair a choice with a working filesystem,
//! and the case worth testing most — a path the picker returned and the
//! filesystem then refused — could not be written at all.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tauri::async_runtime::channel;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// What one turn at the file picker produced.
///
/// Three outcomes rather than a `Result<Option<_>>`, because the caller acts on
/// all three differently and only one of them is a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    /// The paths the person chose, in the order the picker gave them.
    Files(Vec<PathBuf>),
    /// They closed the picker without choosing. Not a failure, and the panel is
    /// told so by an empty answer rather than by an error.
    Cancelled,
    /// Nothing usable came back: the picker could not be opened at all, or what
    /// it answered with is not a file on this machine. The string is the host's
    /// own words for which, for the diagnostics.
    Unavailable(String),
}

/// Where "which files does the person want to attach?" is asked.
///
/// A port because the answer comes off a window the operating system owns: it
/// needs a window server and somebody to click it, so no test could reach it.
/// It answers with plain paths rather than the dialog plugin's own `FilePath`,
/// which a test cannot fabricate and which would relocate the dependency rather
/// than isolate it.
pub trait FilePicker: Send + Sync {
    /// Asks once, and waits as long as the person takes.
    ///
    /// Boxed rather than `impl Future` because composition holds the picker
    /// behind a pointer, and a trait the bundle can carry has to be object
    /// safe. The implementation clones what it needs before the future starts,
    /// so the future borrows nothing from the picker.
    fn choose(&self) -> Pin<Box<dyn Future<Output = Picked> + Send>>;
}

/// The real picker: the operating system's file dialog, through the plugin.
struct NativePicker(AppHandle);

impl FilePicker for NativePicker {
    fn choose(&self) -> Pin<Box<dyn Future<Output = Picked> + Send>> {
        let app = self.0.clone();
        Box::pin(async move {
            // The callback form rather than the blocking one: the blocking call
            // must not run on the main thread, which is where the dialog itself
            // has to be shown, and a channel of one is the whole of what is
            // needed to wait for an answer that arrives exactly once.
            let (answered, mut answer) = channel(1);
            app.dialog().file().pick_files(move |chosen| {
                // Nothing to do if the receiver has gone: the request it
                // belonged to is over.
                let _ = answered.try_send(chosen);
            });

            let Some(chosen) = answer.recv().await else {
                // The callback was dropped without being called, so the picker
                // never got as far as asking anybody anything.
                return Picked::Unavailable("the file picker did not open".to_string());
            };
            let Some(chosen) = chosen else {
                return Picked::Cancelled;
            };

            let mut paths = Vec::with_capacity(chosen.len());
            for file in chosen {
                match file.into_path() {
                    Ok(path) => paths.push(path),
                    // A `file://` URL that is not a path on this machine. The
                    // desktop picker answers with paths, so this is the case
                    // that should not happen — and it is said rather than
                    // skipped, because a selection quietly one file short is
                    // worse than one that was refused.
                    Err(error) => {
                        return Picked::Unavailable(format!(
                            "the file picker answered with something that is not a file on this \
                             machine: {error}"
                        ))
                    }
                }
            }
            Picked::Files(paths)
        })
    }
}

/// The picker this build asks. Called from composition.
pub fn file_picker(app: &AppHandle) -> Arc<dyn FilePicker> {
    Arc::new(NativePicker(app.clone()))
}
