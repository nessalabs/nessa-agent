use std::fmt;

use super::pinned_release::PinRejected;

/// A relative path to one file inside an archive.
///
/// The invariant is the reason this is a type: an archive is downloaded from
/// the network and unpacked into a directory Nessa owns, so a path that is
/// absolute or walks upward out of that directory is a way to write anywhere
/// the app can write. Rejecting those shapes at construction means the unpacker
/// cannot be handed one.
///
/// It is also the path the file is *installed* at, below the directory that
/// holds one artifact — see [`ReleaseContents`] for why the archive's own
/// layout is reproduced rather than flattened.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArchivePath(String);

impl ArchivePath {
    /// Read a path that stays inside the archive it describes.
    ///
    /// One separator is allowed, `/`, which is the one a tar entry uses. A
    /// backslash is refused rather than treated as a separator: accepting it
    /// would mean every reader of this value had to agree on which characters
    /// divide the segments, and the unpacker, the installed file's name and
    /// this type would each have had to be taught the same rule.
    ///
    /// A colon goes with it. `C:evil` is one legal Unix filename and a
    /// drive-relative path on Windows, where joining it onto a directory
    /// replaces the directory rather than extending it.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        let contained = !value.is_empty()
            && !value.starts_with('/')
            && !value.contains(['\\', '\0', ':'])
            && value
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if contained {
            Ok(Self(value.to_owned()))
        } else {
            Err(PinRejected::FilePath(value.to_owned()))
        }
    }

    /// The path as written, with `/` separators.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Every directory this path is inside, outermost first, as written.
    ///
    /// `package/bin/opencode` is inside `package` and then `package/bin`. The
    /// store creates exactly these below the directory that holds one artifact,
    /// and makes exactly these durable, so the chain is derived from the pin
    /// here rather than read out of the archive's own entries — an archive can
    /// carry a directory entry saying anything at all, and none of it decides
    /// where a file lands.
    ///
    /// Path semantics belong to the value object that holds the invariant, not
    /// to whichever adapter needs them.
    pub fn directories(&self) -> Vec<&str> {
        self.0
            .match_indices('/')
            .map(|(at, _)| &self.0[..at])
            .collect()
    }

    /// Whether this path names a directory that `other` is inside.
    ///
    /// Compared with the separator attached rather than as a bare prefix, so
    /// that `bin/codex` is not read as a directory of `bin/codex-acp`. Two
    /// entries in that relationship cannot both exist — one name would have to
    /// be a file and a directory at once — so a pin naming both is refused
    /// before an install discovers it half way through.
    pub fn is_directory_of(&self, other: &Self) -> bool {
        other.0.starts_with(&format!("{}/", self.0))
    }
}

impl fmt::Display for ArchivePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What one file of a release is for.
///
/// Two decisions come off this and no others: which file is handed back as the
/// thing to launch, and which files are installed executable. Modelled as one
/// value rather than two flags because the combinations the flags allow are not
/// all real — a launch target that is not executable is not a launch target —
/// and because "what is this file for" is the question the pin is actually
/// answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileRole {
    /// The program Nessa launches. Exactly one file in a release has this role.
    Launch,
    /// A program the runtime launches for itself. Codex ships four executables
    /// and runs three of them: a code-mode host, the ripgrep it searches with,
    /// and a zsh it runs shell commands under. Installed executable, never
    /// handed out as the launch.
    Helper,
    /// A file that is read rather than run — a package manifest, a licence, a
    /// readme. Installed unreadable by anyone but this user and with no
    /// execute bit, so that a release cannot smuggle in a program by calling
    /// it a document.
    Document,
}

impl FileRole {
    /// The spelling used in the pin file and in the store's own record.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Helper => "helper",
            Self::Document => "document",
        }
    }

    /// Read one of the three spellings, refusing anything else.
    pub fn parse(value: &str) -> Result<Self, PinRejected> {
        match value {
            "launch" => Ok(Self::Launch),
            "helper" => Ok(Self::Helper),
            "document" => Ok(Self::Document),
            other => Err(PinRejected::Contents(format!(
                "{other:?} is not a role a release file can have"
            ))),
        }
    }

    /// Whether a file in this role is installed as something that can be run.
    pub fn runnable(self) -> bool {
        matches!(self, Self::Launch | Self::Helper)
    }
}

impl fmt::Display for FileRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One file a release installs, and what it is for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseFile {
    // Ordered path first, so that deriving `Ord` sorts a release's files by
    // where they go rather than by what they are for.
    path: ArchivePath,
    role: FileRole,
}

impl ReleaseFile {
    pub fn new(path: ArchivePath, role: FileRole) -> Self {
        Self { path, role }
    }

    pub fn path(&self) -> &ArchivePath {
        &self.path
    }

    pub fn role(&self) -> FileRole {
        self.role
    }
}

/// Every file one release installs, with the one Nessa launches named among
/// them.
///
/// A release used to be a single file, because the first runtime pinned here
/// was one: Opencode's archive holds one program and nothing else it needs.
/// Neither of the other two is like that. `@openai/codex-darwin-arm64` holds
/// seven files, four of them programs, and the three it is not launched as are
/// found by the fourth *through the directory it sits in* — ripgrep at
/// `../codex-path/rg`, a zsh under `../codex-resources/`. Unpacking only the
/// program Nessa starts would install something that starts and then cannot
/// search or run a command.
///
/// So the archive's own layout is reproduced below the directory that holds one
/// artifact, rather than every file being flattened into it. Which files that
/// is remains the *pin's* statement and never the archive's: the paths here are
/// what the install writes and what it later checks, so an archive carrying an
/// extra entry installs nothing extra, and one carrying a different name for a
/// pinned file installs nothing at all.
///
/// Held in one value object rather than as a list beside a launch path because
/// the rules that matter span the whole set: one launch and no more, no two
/// files fighting over one path, and no path that is a directory of another.
/// Each of those is unstateable about a file on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseContents {
    /// Sorted by path, so that two pins naming the same files in a different
    /// order are the same contents. The store compares a record against this,
    /// and an ordering difference that read as "something else is installed"
    /// would cost a re-download of a few hundred megabytes to reach exactly the
    /// state already on the disk.
    files: Vec<ReleaseFile>,
    /// Where [`FileRole::Launch`] sits in `files`. Derived at construction
    /// rather than searched for again, so that "exactly one launch" is checked
    /// once and nothing downstream has a missing-launch case to handle.
    launch: usize,
}

impl ReleaseContents {
    /// Describe what a release installs, refusing a set that cannot be one.
    ///
    /// Four rules, and each of them is a pin that would otherwise fail part way
    /// through an install on somebody's machine rather than in the test suite:
    ///
    /// - **Exactly one launch.** None, and there is nothing to hand back to
    ///   start; two, and which of them Nessa starts would depend on the order
    ///   the pin file happens to list them in.
    /// - **No path twice.** Two entries naming one path are two claims about
    ///   one installed file — including two different roles, which is two
    ///   answers to whether it is executable.
    /// - **No path inside another.** `bin` and `bin/codex` cannot both exist:
    ///   one name would have to be a file and a directory at once, and which
    ///   error the install reported would depend on which it wrote first.
    /// - **Not empty**, which the first rule already implies and which is worth
    ///   naming separately because an empty release is the shape a generator
    ///   bug produces.
    pub fn new(files: Vec<ReleaseFile>) -> Result<Self, PinRejected> {
        if files.is_empty() {
            return Err(PinRejected::Contents(
                "a release installs at least one file".to_owned(),
            ));
        }
        let mut files = files;
        files.sort();
        for (index, file) in files.iter().enumerate() {
            for earlier in &files[..index] {
                if earlier.path() == file.path() {
                    return Err(PinRejected::Contents(format!(
                        "{} is named twice",
                        file.path()
                    )));
                }
                if earlier.path().is_directory_of(file.path())
                    || file.path().is_directory_of(earlier.path())
                {
                    return Err(PinRejected::Contents(format!(
                        "{} and {} cannot both be files",
                        earlier.path(),
                        file.path()
                    )));
                }
            }
        }
        let mut launches = files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.role() == FileRole::Launch);
        let Some((launch, _)) = launches.next() else {
            return Err(PinRejected::Contents(
                "a release names no file to launch".to_owned(),
            ));
        };
        if let Some((_, second)) = launches.next() {
            return Err(PinRejected::Contents(format!(
                "a release names two files to launch, {} and {}",
                files[launch].path(),
                second.path()
            )));
        }
        Ok(Self { files, launch })
    }

    /// The program Nessa launches.
    pub fn launch(&self) -> &ArchivePath {
        self.files[self.launch].path()
    }

    /// Every file this release installs, ordered by where it goes.
    pub fn files(&self) -> &[ReleaseFile] {
        &self.files
    }
}

impl fmt::Display for ReleaseContents {
    /// The launch, and how many other files come with it.
    ///
    /// For the one place a release's contents reach a person: a diagnostic
    /// about an archive that did not hold what was pinned. Listing seven paths
    /// there would bury the one that matters.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.files.len() {
            1 => write!(f, "{}", self.launch()),
            count => write!(f, "{} and {} other files", self.launch(), count - 1),
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/release_contents.rs"]
mod tests;
