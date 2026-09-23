//! Getting an agent's own runtime onto this machine.
//!
//! Nessa drives coding agents it does not write, and fetching one is how any of
//! them arrives.
//!
//! Opencode was the first, on a reason that turned out to be false: it was
//! taken to be the agent a first-time user could reach with nothing signed in.
//! Its free models are refused outside OpenCode's own application, so it needs
//! an account like the other two, and there is nothing special about it. What
//! is left is the part that was always true — telling somebody to go and
//! install something before they can use this is the thing this context exists
//! to avoid — and that applies to every agent equally. See
//! `docs/adr/todo/173-fetch-agent-runtimes.md`.
//!
//! A release is not always one file, and that is the other thing this context
//! learned late. Opencode's archive holds one program and nothing the program
//! needs. Codex's holds seven files, four of them programs, and the three it is
//! not launched as are found by the fourth *through the directory it sits in* —
//! a ripgrep at `../codex-path/rg`, a zsh under `../codex-resources/`. So a pin
//! names a set of files rather than one, each with what it is for, and the
//! archive's own layout is reproduced below the directory that holds one
//! artifact. `ReleaseContents` is where that set and its rules live.
//!
//! What is deliberately *not* here: keeping a runtime up to date. Nessa
//! installs the version it has tested and leaves it there. An agent that
//! updated itself underneath a tested pin would make "the version Nessa tested"
//! untrue without anything having changed on our side.
//!
//! Which side owns the launch path, since two could: this one does. The store
//! is asked, at the moment a runtime is about to be launched, for the pinned
//! release — `RuntimeStore::installed` answers from a record it wrote, and
//! answers "not installed" for anything that does not describe exactly the
//! artifact the current pin names, or whose files are not all still there. A
//! launcher that instead kept an absolute path from an earlier install would
//! keep a *working* one: superseded artifacts are left where they are, so the
//! file stays launchable after the pin moves, and the agent Nessa tested would
//! be silently replaced by one it did not. The path `nessa install-agent`
//! prints is therefore a report of what just happened, for a person and for a
//! caller deciding what to say next. It is not a handle to be stored and
//! launched from later.
//!
//! ```text
//! composition -> infrastructure::releases_for    (which release, if any)
//!             -> application::InstallAgentRuntime -> application::ArchiveSource
//!                                                 -> application::RuntimeStore
//! infrastructure::HttpsArchives ------------------> ArchiveSource
//! infrastructure::ManagedRuntimes ----------------> RuntimeStore
//! domain::PinnedRelease --------------------------> what is allowed to be installed
//!          └─ domain::ReleaseContents ------------> which files, and what each is for
//! ```
//! Arrows mean construction, calls, or "is made of". Dependencies point inward:
//! the domain knows nothing about the network or the disk, and the application
//! knows only the two ports.
pub mod application;
pub mod domain;
pub mod infrastructure;
