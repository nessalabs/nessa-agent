//! Linux-GNU acceptance of the actual executable's bootstrap wiring. The loaded
//! fixture's destructor observes only its own synthetic --help process at exit.
#![cfg(all(target_os = "linux", target_env = "gnu"))]
use std::{fs, process::Command};

fn parent_state() -> (libc::rlim_t, libc::rlim_t, libc::c_int) {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: valid writable output for this test process, with no mutation.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) }, 0);
    // SAFETY: reads only this test process's dumpability.
    let dumpable =
        unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0_usize, 0_usize, 0_usize, 0_usize) };
    assert!(dumpable >= 0);
    (limit.rlim_cur, limit.rlim_max, dumpable)
}

#[test]
fn executable_bootstrap_protects_its_process_before_help_dispatch() {
    let parent_before = parent_state();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("memory_receipt.c");
    let library = root.path().join("memory_receipt.so");
    let receipt = root.path().join("receipt");
    fs::write(
        &source,
        r#"
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/resource.h>
__attribute__((destructor)) static void memory_receipt(void) {
    const char *path = getenv("NESSA_MEMORY_TEST_RECEIPT");
    if (!path) return;
    struct rlimit limit;
    int protected = getrlimit(RLIMIT_CORE, &limit) == 0 &&
        limit.rlim_cur == 0 && limit.rlim_max == 0 &&
        prctl(PR_GET_DUMPABLE, 0L, 0L, 0L, 0L) == 0;
    FILE *file = fopen(path, "w");
    if (file) { fputs(protected ? "protected\n" : "unprotected\n", file); fclose(file); }
}
"#,
    )
    .unwrap();
    let compiled = Command::new("cc")
        .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(&source)
        .output()
        .expect(
            "Linux-GNU bootstrap acceptance requires the C compiler used by native dependencies",
        );
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let child = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .arg("--help")
        .env("NESSA_STAGE", "ci")
        .env("NESSA_DATA_DIR", root.path())
        .env("LD_PRELOAD", &library)
        .env("NESSA_MEMORY_TEST_RECEIPT", &receipt)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(String::from_utf8_lossy(&child.stdout).starts_with("Nessa\n"));
    assert_eq!(fs::read_to_string(receipt).unwrap(), "protected\n");
    assert_eq!(
        parent_state(),
        parent_before,
        "the parent test's native state must remain unchanged"
    );
}
