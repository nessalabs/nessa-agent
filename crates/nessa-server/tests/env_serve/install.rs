//! The commands that find, put and start this build on a host, run by this
//! machine's own `sh` exactly as `sshd` hands them to a login shell, against
//! a home directory of their own: so each operating system CI runs on
//! (Linux and macOS) checks its own `uname`, digest tool, `mktemp`, `find`
//! and `mv`. The real binary's install and start are in
//! `tests/env_serve_binary.rs`.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const PROTOCOL: &str = "0123456789abcdef";

/// Run `command` as `sshd` would: the login shell (`sh` here) is handed it
/// whole with `-c`, and `HOME` is `home`.
fn on_host(home: &Path, command: &str, input: Option<&Path>) -> String {
    let stdin = match input {
        Some(path) => Stdio::from(std::fs::File::open(path).unwrap()),
        None => Stdio::null(),
    };
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap())
        .stdin(stdin)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap()
}

/// A stand-in build: a script that answers `env protocol` with `protocol`,
/// and its SHA-256.
fn build(directory: &Path, protocol: &str, runs: bool) -> (PathBuf, String) {
    let path = directory.join("nessa");
    let body = if runs {
        format!("#!/bin/sh\n[ \"$1 $2\" = \"env protocol\" ] && echo {protocol}\n")
    } else {
        "#!/bin/sh\nexit 1\n".to_owned()
    };
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(body.as_bytes()).unwrap();
    let digest = format!("{:x}", Sha256::digest(body.as_bytes()));
    (path, digest)
}

fn installed(home: &Path) -> PathBuf {
    home.join(INSTALL_DIRECTORY).join(PROTOCOL).join("nessa")
}

/// Nothing an install leaves behind but the installed copy.
fn leftovers(home: &Path) -> Vec<String> {
    match std::fs::read_dir(home.join(INSTALL_DIRECTORY)) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".install."))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn every_command_survives_any_login_shell() {
    let digest = "a".repeat(64);
    for command in [
        serve_command(PROTOCOL),
        probe_command(PROTOCOL),
        upload_command(PROTOCOL, &digest),
    ] {
        let script = command
            .strip_prefix("sh -c '")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("one single-quoted word for sh");
        for forbidden in ['\'', '!', '\\', '\n', '\r'] {
            assert!(!script.contains(forbidden), "{forbidden:?} in {command}");
        }
    }
}

#[test]
#[should_panic(expected = "only hex goes into a host command")]
fn nothing_but_hex_goes_into_a_command() {
    let _ = serve_command("0123'; rm -rf ~; '");
}

#[test]
fn a_host_without_this_build_says_what_it_runs_on_and_this_build_runs_there() {
    let home = tempfile::tempdir().unwrap();
    let probe = Probe::parse(&on_host(home.path(), &probe_command(PROTOCOL), None)).unwrap();
    let Probe::Absent(platform) = probe else {
        panic!("nothing is installed in an empty home: {probe:?}")
    };
    assert_eq!(platform.os, Platform::this_build().os);
    assert_eq!(platform.arch, Platform::this_build().arch);
    assert!(Platform::this_build().runs_on(&platform), "{platform}");
}

#[test]
fn an_upload_that_verifies_is_installed_where_the_probe_and_serve_look() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, true);
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(Upload::parse(&answer), Some(Upload::Installed), "{answer}");
    assert_eq!(
        std::fs::read(installed(home.path())).unwrap(),
        std::fs::read(&path).unwrap()
    );
    let mode = std::fs::metadata(installed(home.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    assert!(leftovers(home.path()).is_empty());
    assert_eq!(
        Probe::parse(&on_host(home.path(), &probe_command(PROTOCOL), None)),
        Some(Probe::Present)
    );
    // The serve command runs exactly that copy.
    assert_eq!(
        on_host(
            home.path(),
            &serve_command(PROTOCOL).replace("env serve", "env protocol"),
            None
        )
        .trim(),
        PROTOCOL
    );
    // A second install of the same bytes replaces it with the same bytes.
    let again = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(Upload::parse(&again), Some(Upload::Installed));
}

#[test]
fn bytes_that_are_not_the_ones_sent_are_refused_and_nothing_is_left() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, true);
    let wrong = "0".repeat(64);
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &wrong), Some(&path));
    assert_eq!(
        Upload::parse(&answer),
        Some(Upload::Refused(UploadRefusal::Fingerprint(digest))),
        "{answer}"
    );
    assert!(!installed(home.path()).exists());
    assert!(leftovers(home.path()).is_empty());
}

#[test]
fn a_copy_cut_short_is_refused_by_its_fingerprint() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, true);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert!(matches!(
        Upload::parse(&answer),
        Some(Upload::Refused(UploadRefusal::Fingerprint(_)))
    ));
    assert!(!installed(home.path()).exists());
}

#[test]
fn a_copy_speaking_another_protocol_is_refused_and_nothing_is_left() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), "fedcba9876543210", true);
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(
        Upload::parse(&answer),
        Some(Upload::Refused(UploadRefusal::Version(
            "fedcba9876543210".into()
        )))
    );
    assert!(!installed(home.path()).exists());
    assert!(leftovers(home.path()).is_empty());
}

#[test]
fn a_copy_that_does_not_run_there_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, false);
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(
        Upload::parse(&answer),
        Some(Upload::Refused(UploadRefusal::Unrunnable))
    );
    assert!(!installed(home.path()).exists());
}

#[test]
fn a_home_that_cannot_hold_it_is_a_failed_step() {
    let home = tempfile::tempdir().unwrap();
    // `.nessa` is a file, so its `env` directory cannot be made.
    std::fs::write(home.path().join(".nessa"), b"").unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, true);
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(
        Upload::parse(&answer),
        Some(Upload::Refused(UploadRefusal::Failed("directory".into())))
    );
}

#[test]
fn a_home_with_spaces_in_its_path_is_used_as_it_is() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("a home with spaces");
    std::fs::create_dir(&home).unwrap();
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, true);
    let answer = on_host(&home, &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(Upload::parse(&answer), Some(Upload::Installed), "{answer}");
    assert!(installed(&home).exists());
}

#[test]
fn probes_and_uploads_read_only_their_own_answers() {
    assert_eq!(Probe::parse("present\n"), Some(Probe::Present));
    assert_eq!(
        Probe::parse("absent Darwin arm64 other\n"),
        Some(Probe::Absent(Platform {
            os: "macos".into(),
            arch: "aarch64".into(),
            libc: "other".into(),
        }))
    );
    assert_eq!(
        Probe::parse("absent Linux amd64 gnu"),
        Some(Probe::Absent(Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: "gnu".into(),
        }))
    );
    assert_eq!(
        Probe::parse("Welcome to the box\npresent\n"),
        Some(Probe::Present)
    );
    for other in ["", "Welcome to the box", "absent Linux", "present twice"] {
        assert_eq!(Probe::parse(other), None, "{other}");
    }
    assert_eq!(Upload::parse("motd\ninstalled\n"), Some(Upload::Installed));
    assert_eq!(
        Upload::parse("refused fingerprint \u{1b}[31mabc"),
        Some(Upload::Refused(UploadRefusal::Fingerprint(
            "[31mabc".into()
        )))
    );
    assert_eq!(
        Upload::parse("refused fingerprint"),
        Some(Upload::Refused(UploadRefusal::Fingerprint(String::new())))
    );
    assert_eq!(
        Upload::parse("refused digest_tool"),
        Some(Upload::Refused(UploadRefusal::NoDigestTool))
    );
    assert_eq!(
        Upload::parse(&format!("refused version {}", "x".repeat(500))),
        Some(Upload::Refused(UploadRefusal::Version("x".repeat(128))))
    );
    for other in ["", "installed now", "failed", "refused"] {
        assert_eq!(Upload::parse(other), None, "{other}");
    }
}

#[test]
fn a_build_runs_only_on_its_own_system_and_processor() {
    let platform = |os: &str, arch: &str, libc: &str| Platform {
        os: os.into(),
        arch: arch.into(),
        libc: libc.into(),
    };
    let linux_gnu = platform("linux", "x86_64", "gnu");
    assert!(linux_gnu.runs_on(&platform("linux", "x86_64", "gnu")));
    assert!(!linux_gnu.runs_on(&platform("linux", "x86_64", "other")));
    assert!(!linux_gnu.runs_on(&platform("linux", "aarch64", "gnu")));
    assert!(!linux_gnu.runs_on(&platform("macos", "x86_64", "other")));
    let linux_static = platform("linux", "x86_64", "other");
    assert!(linux_static.runs_on(&platform("linux", "x86_64", "other")));
    let mac = platform("macos", "aarch64", "other");
    assert!(mac.runs_on(&platform("macos", "aarch64", "other")));
    assert!(!mac.runs_on(&platform("macos", "x86_64", "other")));
}

/// A copy that is there but no longer runs, or says another protocol, is
/// absent to the probe, so it is installed again.
#[test]
fn a_copy_that_no_longer_runs_or_speaks_another_protocol_is_absent() {
    let home = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    for (protocol, runs) in [(PROTOCOL, false), ("fedcba9876543210", true)] {
        let (path, _) = build(source.path(), protocol, runs);
        std::fs::create_dir_all(installed(home.path()).parent().unwrap()).unwrap();
        // Written by a child, as an upload is: a file this test process held
        // open for writing while another test forked could not be run.
        let placed = Command::new("sh")
            .arg("-c")
            .arg("cat > \"$1\" && chmod 700 \"$1\"")
            .arg("sh")
            .arg(installed(home.path()))
            .stdin(std::fs::File::open(&path).unwrap())
            .status()
            .unwrap();
        assert!(placed.success());
        assert!(matches!(
            Probe::parse(&on_host(home.path(), &probe_command(PROTOCOL), None)),
            Some(Probe::Absent(_))
        ));
    }
    let (path, digest) = build(source.path(), PROTOCOL, true);
    let answer = on_host(home.path(), &upload_command(PROTOCOL, &digest), Some(&path));
    assert_eq!(Upload::parse(&answer), Some(Upload::Installed));
    assert_eq!(
        Probe::parse(&on_host(home.path(), &probe_command(PROTOCOL), None)),
        Some(Probe::Present)
    );
}

/// Each command reaches `sh` whole through every login shell this machine
/// has, as `sshd` hands it over: `<shell> -c <command>`. On Linux and macOS
/// CI that is bash, and zsh and tcsh where installed (macOS has both).
#[test]
fn every_command_runs_the_same_through_each_login_shell_here() {
    let shells: Vec<_> = ["bash", "zsh", "fish", "tcsh", "dash", "ksh"]
        .iter()
        .flat_map(|shell| ["/bin", "/usr/bin"].map(|directory| Path::new(directory).join(shell)))
        .filter(|path| path.exists())
        .collect();
    assert!(!shells.is_empty());
    let source = tempfile::tempdir().unwrap();
    let (path, digest) = build(source.path(), PROTOCOL, true);
    for shell in shells {
        let home = tempfile::tempdir().unwrap();
        let run = |command: &str, input: Option<&Path>| {
            let stdin = match input {
                Some(path) => Stdio::from(std::fs::File::open(path).unwrap()),
                None => Stdio::null(),
            };
            let output = Command::new(&shell)
                .arg("-c")
                .arg(command)
                .env_clear()
                .env("HOME", home.path())
                .env("PATH", std::env::var_os("PATH").unwrap())
                .stdin(stdin)
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        let shown = shell.display();
        assert!(
            matches!(
                Probe::parse(&run(&probe_command(PROTOCOL), None)),
                Some(Probe::Absent(_))
            ),
            "{shown}"
        );
        let answer = run(&upload_command(PROTOCOL, &digest), Some(&path));
        assert_eq!(
            Upload::parse(&answer),
            Some(Upload::Installed),
            "{shown}: {answer}"
        );
        assert_eq!(
            Probe::parse(&run(&probe_command(PROTOCOL), None)),
            Some(Probe::Present),
            "{shown}"
        );
    }
}
