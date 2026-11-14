use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};
fn installed(home: &Path, digest: &str) -> std::path::PathBuf {
    home.join(nessa_env_serve::install::INSTALL_DIRECTORY)
        .join(digest)
        .join("nessa")
}
fn on_host(home: &Path, command: &str, input: Option<&Path>) -> String {
    let output = Command::new("sh")
        .args(["-c", command])
        .env("HOME", home)
        .current_dir(home)
        .stdin(input.map_or_else(Stdio::null, |path| {
            Stdio::from(std::fs::File::open(path).unwrap())
        }))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn background_arguments_survive_the_login_shell_without_interpretation_or_newline_loss() {
    let home = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let program = installed(home.path(), &digest);
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::write(&program, "#!/bin/sh\nprintf '%s\\000' \"$@\"\ncat\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = "/tmp/space ' $(touch SHOULD_NOT_EXIST)\n";
    let text = "printf 'quoted'; echo \"$HOME\"; `false`\n\n";
    let command = command(&[&digest], cwd, 42, text);
    let input = home.path().join("stdin");
    std::fs::write(&input, "still open").unwrap();
    let output = on_host(home.path(), &command, Some(&input));
    assert_eq!(
        output.split('\0').collect::<Vec<_>>(),
        [
            "background-run",
            "--cwd",
            cwd,
            "--timeout",
            "42",
            "--",
            "bash",
            "--noprofile",
            "--norc",
            "-c",
            text,
            "still open"
        ]
    );
    assert!(!home.path().join("SHOULD_NOT_EXIST").exists());
}
