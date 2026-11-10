//! The SSH helper's actual CLI, over anonymous pipes, with no VM or credential.
#![cfg(unix)]
use std::{
    io::Read,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn background_run_exits_with_stdin_open_and_clears_agent_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args([
            "background-run",
            "--cwd",
            directory.path().to_str().unwrap(),
            "--timeout",
            "10",
            "--",
            "bash",
            "--noprofile",
            "--norc",
            "-c",
            "printf result; printf err >&2; printf '%s' \"$BACKGROUND_TEST_SECRET\"; exit 7",
        ])
        .env("NESSA_DATA_DIR", directory.path())
        .env("NESSA_STAGE", "ci")
        .env("BACKGROUND_TEST_SECRET", "must-not-reach-command")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _keep_input = child.stdin.take().unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("background helper did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["cause"], "exited");
    assert_eq!(value["exit_code"], 7);
    assert_eq!(value["stdout"], "result");
    assert_eq!(value["stderr"], "err");
    assert_eq!(value["cleanup_verified"], true);
    assert!(child.wait().unwrap().success());
}

#[test]
fn background_run_preserves_a_large_record_on_shared_socket_stdio() {
    let directory = tempfile::tempdir().unwrap();
    let (mut parent, socket) = UnixStream::pair().unwrap();
    let capacity: libc::c_int = 4096;
    // SAFETY: the owned socket and integer option are valid for the call.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&capacity as *const libc::c_int).cast(),
                std::mem::size_of_val(&capacity) as libc::socklen_t,
            )
        },
        0
    );
    let input: OwnedFd = socket.try_clone().unwrap().into();
    let output: OwnedFd = socket.into();
    parent
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args([
            "background-run",
            "--cwd",
            directory.path().to_str().unwrap(),
            "--timeout",
            "10",
            "--",
            "bash",
            "--noprofile",
            "--norc",
            "-c",
            "head -c 100000 /dev/zero",
        ])
        .env("NESSA_DATA_DIR", directory.path())
        .env("NESSA_STAGE", "ci")
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut readable = libc::pollfd {
        fd: parent.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialized pollfd remains live for this bounded wait.
    let ready = unsafe { libc::poll(&mut readable, 1, 15_000) };
    if ready <= 0 {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("helper wrote no record");
    }
    // Hold the reader off while a record larger than the socket buffer is out.
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        child.try_wait().unwrap().is_none(),
        "helper exited before the blocked record was drained"
    );
    let mut bytes = Vec::new();
    let read = parent.read_to_end(&mut bytes);
    if read.is_err() {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    read.unwrap();
    assert!(child.wait().unwrap().success());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["cause"], "exited");
    assert_eq!(value["cleanup_verified"], true);
    assert_eq!(value["stdout"].as_str().unwrap().len(), 16 * 1024);
    assert!(value["omitted_bytes"].as_u64().unwrap() > 0);
}
