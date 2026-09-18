use super::*;

/// Install the pinned Opencode release the way the command really does it:
/// through the async runtime this process starts with.
///
/// Ignored by default alongside the other live test — it fetches the real
/// archive. It exists because the unit tests cannot catch the one thing that is
/// only true at runtime: the HTTP client used here refuses to run inside a
/// Tokio runtime, so `install` being reached through `spawn_blocking` rather
/// than directly is load-bearing, and a refactor that inlined it would panic in
/// production while every other test stayed green.
///
/// ```text
/// cargo test -p nessa-server --lib -- --ignored installs_from_inside_the_runtime
/// ```
#[test]
#[ignore = "downloads the real release archive"]
fn installs_from_inside_the_runtime() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("an async runtime");

    let installed = runtime
        .block_on(async {
            // Spawned from inside the runtime, as `execute` does it. Building
            // the task outside `block_on` would be a different thing entirely —
            // `spawn_blocking` needs a runtime context to be called at all.
            let root = root.path().to_owned();
            tokio::task::spawn_blocking(move || install("opencode", &root)).await
        })
        .expect("the installer finishes")
        .expect("the pinned release installs");

    assert_eq!(installed["agent"], "opencode");
    assert_eq!(installed["downloaded"], true);
    assert!(std::path::Path::new(
        installed["executable"]
            .as_str()
            .expect("an executable path")
    )
    .is_file());
}

#[test]
fn an_agent_nessa_does_not_install_is_named_as_such() {
    // Claude and Codex are expected to be on the machine already. Asking to
    // install one is a mistake worth a clear answer rather than a download that
    // fails obscurely — and it must not touch the network to say so.
    let root = tempfile::tempdir().expect("temporary root");
    let failure = install("claude", root.path()).expect_err("claude is not installed by nessa");
    assert!(
        failure.to_string().contains("not an agent nessa installs"),
        "unhelpful message: {failure}"
    );
}

#[test]
fn nothing_is_written_for_an_agent_with_no_release() {
    let root = tempfile::tempdir().expect("temporary root");
    let _ = install("claude", root.path());
    assert!(
        !root.path().join("claude").exists(),
        "a refused install left a directory behind"
    );
}
