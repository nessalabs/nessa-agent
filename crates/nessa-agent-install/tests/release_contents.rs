use super::*;

/// A file at `path` in `role`, for a set being assembled.
fn file(path: &str, role: FileRole) -> ReleaseFile {
    ReleaseFile::new(ArchivePath::parse(path).expect("a contained path"), role)
}

/// The shape Codex has: one program, three helpers, three documents, in an
/// order nobody would write them in.
fn codex() -> Vec<ReleaseFile> {
    vec![
        file(
            "package/vendor/aarch64-apple-darwin/codex-path/rg",
            FileRole::Helper,
        ),
        file("package/package.json", FileRole::Document),
        file(
            "package/vendor/aarch64-apple-darwin/bin/codex",
            FileRole::Launch,
        ),
        file(
            "package/vendor/aarch64-apple-darwin/codex-resources/zsh/bin/zsh",
            FileRole::Helper,
        ),
    ]
}

#[test]
fn a_path_stays_inside_the_archive() {
    for contained in [
        "package/bin/opencode",
        "package/claude",
        "package/vendor/aarch64-apple-darwin/codex-path/rg",
        "a",
    ] {
        assert!(
            ArchivePath::parse(contained).is_ok(),
            "{contained:?} is one file below the archive"
        );
    }
    for escapes in [
        // Absolute, so joining it onto the artifact directory replaces it.
        "/etc/passwd",
        // Walks upward out of wherever it is joined onto.
        "../outside",
        "package/../../outside",
        "package/..",
        // Empty segments leave this type and the unpacker free to disagree
        // about where the segments divide.
        "",
        "package//bin",
        "package/./bin",
        // A backslash is a separator on Windows and an ordinary character
        // here, and a colon is drive-relative there.
        "package\\bin",
        "c:evil",
        "package/bin\0",
    ] {
        assert_eq!(
            ArchivePath::parse(escapes),
            Err(PinRejected::FilePath(escapes.to_string())),
            "{escapes:?} does not name one file below the archive"
        );
    }
}

#[test]
fn a_path_names_every_directory_it_is_inside_outermost_first() {
    // What the store creates below the artifact directory, and makes durable
    // from the inside out. Derived from the pin so that no directory entry in
    // a downloaded archive decides where anything appears.
    let path = ArchivePath::parse("package/vendor/aarch64-apple-darwin/bin/codex")
        .expect("a contained path");
    assert_eq!(
        path.directories(),
        vec![
            "package",
            "package/vendor",
            "package/vendor/aarch64-apple-darwin",
            "package/vendor/aarch64-apple-darwin/bin",
        ]
    );
    let flat = ArchivePath::parse("claude").expect("a contained path");
    assert!(
        flat.directories().is_empty(),
        "a file at the top of the archive is inside nothing"
    );
}

#[test]
fn a_directory_is_only_one_when_the_separator_is_there_too() {
    let codex = ArchivePath::parse("bin/codex").expect("a contained path");
    let acp = ArchivePath::parse("bin/codex-acp").expect("a contained path");
    let inside = ArchivePath::parse("bin/codex/helper").expect("a contained path");
    // The case a bare `starts_with` gets wrong: one name is a prefix of the
    // other and neither contains the other.
    assert!(!codex.is_directory_of(&acp));
    assert!(!acp.is_directory_of(&codex));
    assert!(codex.is_directory_of(&inside));
    assert!(!inside.is_directory_of(&codex));
    assert!(!codex.is_directory_of(&codex));
}

#[test]
fn a_role_is_one_of_three_spellings() {
    for (spelling, role) in [
        ("launch", FileRole::Launch),
        ("helper", FileRole::Helper),
        ("document", FileRole::Document),
    ] {
        assert_eq!(FileRole::parse(spelling), Ok(role));
        assert_eq!(role.as_str(), spelling);
    }
    // Refused rather than treated as a document. A misspelling that quietly
    // became the least privileged role would install Codex's ripgrep unable to
    // run, and the failure would surface as a search that does not work.
    assert!(matches!(
        FileRole::parse("Launch"),
        Err(PinRejected::Contents(_))
    ));
    assert!(matches!(
        FileRole::parse("executable"),
        Err(PinRejected::Contents(_))
    ));
}

#[test]
fn only_a_launch_or_a_helper_is_installed_runnable() {
    assert!(FileRole::Launch.runnable());
    assert!(FileRole::Helper.runnable());
    // The whole point of the third role: Codex ships three files that are read
    // rather than run, and the archive's own mode bits do not get a say.
    assert!(!FileRole::Document.runnable());
}

#[test]
fn contents_are_sorted_by_where_the_files_go() {
    // So that two pins naming the same files in a different order are the same
    // contents. The store compares a record against this, and an ordering
    // difference reading as "something else is installed" would cost a
    // re-download of a few hundred megabytes to reach the state already there.
    let contents = ReleaseContents::new(codex()).expect("a package of four files");
    let mut shuffled = codex();
    shuffled.reverse();
    assert_eq!(
        contents,
        ReleaseContents::new(shuffled).expect("the same four files")
    );
    assert_eq!(
        contents
            .files()
            .iter()
            .map(|file| file.path().as_str())
            .collect::<Vec<_>>(),
        vec![
            "package/package.json",
            "package/vendor/aarch64-apple-darwin/bin/codex",
            "package/vendor/aarch64-apple-darwin/codex-path/rg",
            "package/vendor/aarch64-apple-darwin/codex-resources/zsh/bin/zsh",
        ]
    );
}

#[test]
fn exactly_one_file_is_the_one_to_launch() {
    let contents = ReleaseContents::new(codex()).expect("a package of four files");
    assert_eq!(
        contents.launch().as_str(),
        "package/vendor/aarch64-apple-darwin/bin/codex"
    );
    // The launch is found among files sorted by path, so it is not the first
    // one — which is exactly the case an index assumed to be zero would break.
    assert_ne!(contents.files()[0].path(), contents.launch());
}

#[test]
fn a_release_with_no_program_to_launch_is_refused() {
    // Nothing to hand back to start. The store would have no path to report
    // and the launcher no file to run, and the failure would arrive long after
    // the download.
    let refusal = ReleaseContents::new(vec![
        file("package/package.json", FileRole::Document),
        file("package/bin/rg", FileRole::Helper),
    ]);
    assert!(matches!(refusal, Err(PinRejected::Contents(_))));
}

#[test]
fn a_release_with_two_programs_to_launch_is_refused() {
    // Which of them Nessa starts would otherwise depend on the order the pin
    // file happens to list them in.
    let refusal = ReleaseContents::new(vec![
        file("package/bin/codex", FileRole::Launch),
        file("package/bin/codex-acp", FileRole::Launch),
    ]);
    assert!(matches!(refusal, Err(PinRejected::Contents(_))));
}

#[test]
fn a_release_that_installs_nothing_is_refused() {
    assert!(matches!(
        ReleaseContents::new(Vec::new()),
        Err(PinRejected::Contents(_))
    ));
}

#[test]
fn one_path_cannot_be_named_twice() {
    // Two entries naming one path are two claims about one installed file —
    // here two different answers to whether it is executable.
    let refusal = ReleaseContents::new(vec![
        file("package/claude", FileRole::Launch),
        file("package/claude", FileRole::Document),
    ]);
    assert!(matches!(refusal, Err(PinRejected::Contents(_))));
    // Including two identical ones, which say the same thing and still leave
    // the install writing the same file twice.
    let repeated = ReleaseContents::new(vec![
        file("package/claude", FileRole::Launch),
        file("package/claude", FileRole::Launch),
    ]);
    assert!(matches!(repeated, Err(PinRejected::Contents(_))));
}

#[test]
fn a_path_cannot_be_a_directory_of_another() {
    // `package/bin` would have to be a file and a directory at once, and which
    // error the install reported would depend on which of the two it wrote
    // first.
    for order in [
        vec![
            file("package/bin", FileRole::Launch),
            file("package/bin/rg", FileRole::Helper),
        ],
        vec![
            file("package/bin/rg", FileRole::Helper),
            file("package/bin", FileRole::Launch),
        ],
    ] {
        assert!(matches!(
            ReleaseContents::new(order),
            Err(PinRejected::Contents(_))
        ));
    }
    // And the near miss stays legal: one name being a prefix of another is not
    // one being inside the other.
    assert!(ReleaseContents::new(vec![
        file("package/bin/codex", FileRole::Launch),
        file("package/bin/codex-code-mode-host", FileRole::Helper),
    ])
    .is_ok());
}

#[test]
fn contents_say_what_is_launched_and_how_much_comes_with_it() {
    // The one place this reaches a person is a diagnostic about an archive
    // that did not hold what was pinned; listing seven paths there would bury
    // the one that matters.
    let one = ReleaseContents::new(vec![file("package/bin/opencode", FileRole::Launch)])
        .expect("one program is a release");
    assert_eq!(one.to_string(), "package/bin/opencode");
    let package = ReleaseContents::new(codex()).expect("a package of four files");
    assert_eq!(
        package.to_string(),
        "package/vendor/aarch64-apple-darwin/bin/codex and 3 other files"
    );
}
