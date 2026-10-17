use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
};
use tempfile::tempdir;

fn acquired(directory: &Path) -> DesktopHostLease {
    match try_claim(directory).unwrap() {
        DesktopHostClaim::Acquired(lease) => lease,
        DesktopHostClaim::Busy => panic!("expected acquired lease"),
    }
}

#[test]
fn competing_bundle_cannot_claim_the_same_service_until_final_release() {
    let root = tempdir().unwrap();
    let first = acquired(root.path());
    let worker = first.clone();
    assert!(matches!(
        try_claim(root.path()).unwrap(),
        DesktopHostClaim::Busy
    ));
    drop(first);
    assert!(matches!(
        try_claim(root.path()).unwrap(),
        DesktopHostClaim::Busy
    ));
    drop(worker);
    let _successor = acquired(root.path());
    assert!(root.path().join("desktop-host.lock").is_file());
}

#[test]
fn different_service_labels_do_not_contend() {
    let root = tempdir().unwrap();
    let _prod = acquired(&root.path().join("so.nessa.gateway.prod"));
    let _ci = acquired(&root.path().join("so.nessa.gateway.ci"));
    let _instance = acquired(&root.path().join("so.nessa.gateway.prod.other"));
}

#[test]
fn a_symlink_lock_is_refused_without_mutating_its_target() {
    let root = tempdir().unwrap();
    let outside = root.path().join("outside");
    std::fs::write(&outside, b"preserve").unwrap();
    let directory = root.path().join("service");
    create_directory(&directory).unwrap();
    std::os::unix::fs::symlink(&outside, directory.join("desktop-host.lock")).unwrap();
    assert!(try_claim(&directory).is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"preserve");
}

struct HeldChild(Child);
impl Drop for HeldChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child(directory: &Path, mode: &str) -> HeldChild {
    let test_module = module_path!().split_once("::").unwrap().1;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{test_module}::ownership_process_child"),
            "--nocapture",
            "--test-threads=1",
        ])
        .env("NESSA_HOST_OWNERSHIP_CHILD", mode)
        .env("NESSA_HOST_OWNERSHIP_DIRECTORY", directory)
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let child = HeldChild(child);
    let (ready, wait_ready) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = BufReader::new(output);
        loop {
            let mut line = String::new();
            if output.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.contains("HOST_OWNERSHIP_CHILD_READY") {
                let _ = ready.send(());
                break;
            }
        }
    });
    let result = wait_ready.recv_timeout(std::time::Duration::from_secs(5));
    if result.is_err() {
        drop(child);
        reader.join().unwrap();
        panic!("child did not publish readiness: {result:?}");
    }
    reader.join().unwrap();
    child
}

#[test]
fn ownership_process_child() {
    let Some(mode) = std::env::var_os("NESSA_HOST_OWNERSHIP_CHILD") else {
        return;
    };
    let directory = std::env::var_os("NESSA_HOST_OWNERSHIP_DIRECTORY").unwrap();
    let _lease = if mode == "claim" {
        Some(acquired(Path::new(&directory)))
    } else {
        None
    };
    println!("HOST_OWNERSHIP_CHILD_READY");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}

#[test]
fn process_death_releases_the_original_stable_lock() {
    let root = tempdir().unwrap();
    let mut owner = child(root.path(), "claim");
    assert!(matches!(
        try_claim(root.path()).unwrap(),
        DesktopHostClaim::Busy
    ));
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    let _successor = acquired(root.path());
}

#[test]
fn an_exec_child_does_not_keep_the_lease_after_its_parent_releases() {
    let root = tempdir().unwrap();
    let lease = acquired(root.path());
    let mut observer = child(root.path(), "observe");
    drop(lease);
    let _successor = acquired(root.path());
    assert!(observer.0.try_wait().unwrap().is_none());
}

#[test]
fn explicit_unlock_releases_an_inherited_fork_description() {
    let root = tempdir().unwrap();
    let lease = acquired(root.path());
    let mut descriptors = [0; 2];
    assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        // Only async-signal-safe syscalls after fork; the child deliberately
        // keeps the inherited lock description until its parent finishes.
        unsafe {
            libc::close(descriptors[1]);
            let mut byte = 0u8;
            libc::read(descriptors[0], &mut byte as *mut u8 as *mut libc::c_void, 1);
            libc::_exit(0);
        }
    }
    struct ForkedChild {
        pid: libc::pid_t,
        finish: i32,
    }
    impl Drop for ForkedChild {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.finish);
                libc::waitpid(self.pid, std::ptr::null_mut(), 0);
            }
        }
    }
    unsafe {
        libc::close(descriptors[0]);
    }
    let _child = ForkedChild {
        pid,
        finish: descriptors[1],
    };
    drop(lease);
    let _successor = acquired(root.path());
}
