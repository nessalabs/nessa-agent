use super::super::generation::random_generation;
use super::{
    begin_stage_measurement, clone_file, end_stage_measurement, entry_name, finalize_file,
    launch_settings, parallelism, publish, stage_runtime, stage_runtime_cached,
    stage_runtime_using, tree_fingerprint, utf8, validate_runtime, CopyHookGuard, StagePhases,
    SyncHookGuard, ValidatedRuntimes,
};
use crate::gateway::application::{RuntimeCopyPhase, RuntimeCopyProgress};
use serde_json::json;
use std::{
    ffi::{CString, OsString},
    fs::{self, OpenOptions, Permissions},
    io::Write,
    os::fd::AsRawFd,
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const TEST_ATTRIBUTE: &[u8] = b"com.nessa.runtime-staging-test\0";

fn set_test_attribute(path: &Path) {
    let path = CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe {
            libc::setxattr(
                path.as_ptr(),
                TEST_ATTRIBUTE.as_ptr().cast(),
                b"untrusted".as_ptr().cast(),
                b"untrusted".len(),
                0,
                0,
            )
        },
        0
    );
}

fn has_test_attribute(path: &Path) -> bool {
    let path = CString::new(path.to_str().unwrap()).unwrap();
    let result = unsafe {
        libc::getxattr(
            path.as_ptr(),
            TEST_ATTRIBUTE.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if result >= 0 {
        true
    } else {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOATTR)
        );
        false
    }
}

fn force_byte_copy(_: &Path, _: &Path) -> Result<bool, String> {
    Ok(false)
}

fn fail_clone_attempt(_: &Path, _: &Path) -> Result<bool, String> {
    Err("forced clone failure".into())
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nessa-runtime-stage-{}",
            random_generation().unwrap()
        ));
        nessa_local_storage::create_directory(&path).unwrap();
        Self(path)
    }
    fn source(&self) -> PathBuf {
        let source = self.0.join("source");
        nessa_local_storage::create_directory(&source).unwrap();
        fs::write(source.join("nessa"), b"gateway\0bytes").unwrap();
        fs::set_permissions(source.join("nessa"), Permissions::from_mode(0o751)).unwrap();
        fs::write(source.join("node"), b"node").unwrap();
        source
    }
    fn manifest(source: &Path) -> String {
        let fingerprint = tree_fingerprint(source).unwrap();
        fs::write(
            source.join("manifest.json"),
            serde_json::to_vec(&json!({"fingerprint":fingerprint})).unwrap(),
        )
        .unwrap();
        fingerprint
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn rust_fingerprint_matches_javascript_with_unicode_and_escape_framing() {
    let fixture = Fixture::new();
    let source = fixture.source();
    for name in ["quote\"\\\n\u{2028}.txt", "\u{e000}", "\u{10000}", "é", "𝄞"] {
        fs::write(source.join(name), name.as_bytes()).unwrap();
    }
    fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("nested/manifest.json"), b"included").unwrap();
    symlink("quote\"\\\n\u{2028}.txt", source.join("link")).unwrap();
    let expected = Fixture::manifest(&source);
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/desktop/runtime-fingerprint.mjs");
    let result=Command::new("node").args(["--input-type=module","-e","const {runtimeFingerprint}=await import(process.argv[1]); process.stdout.write(runtimeFingerprint(process.argv[2]));"]).arg(script).arg(&source).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
    let staged = stage_runtime(&source, &fixture.0.join("versions"), &expected).unwrap();
    assert_eq!(tree_fingerprint(&staged).unwrap(), expected);
}
#[test]
fn published_runtime_is_private_reusable_immutable_and_definition_uses_staged_paths() {
    let fixture = Fixture::new();
    let source = fixture.source();
    symlink("nessa", source.join("gateway-link")).unwrap();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let staged = stage_runtime(&source, &versions, &fingerprint).unwrap();
    assert_eq!(staged, versions.join(&fingerprint));
    assert_eq!(
        fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(staged.join("nessa"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o711
    );
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    assert_eq!(
        fs::read_link(staged.join("gateway-link")).unwrap(),
        Path::new("nessa")
    );
    // The service runs the staged copy, never the bundle it was staged from,
    // and it is addressed absolutely: no search path is derived from it.
    //
    // The other half of this is `what_launchd_is_configured_to_run_resolves_to
    // _a_managed_launch` in `crates/nessa-server/src/core/launch.rs`: these
    // arguments are what tells the server it is the service launchd supervises
    // rather than something someone typed, and the two crates share no
    // dependency to state it in one place.
    assert_eq!(
        launch_settings(&staged),
        json!([staged.join("nessa"), "server", "--desktop-runtime", staged])
    );
    assert!(!launch_settings(&staged)
        .to_string()
        .contains(source.to_str().unwrap()));
    assert_eq!(
        stage_runtime(&source, &versions, &fingerprint).unwrap(),
        staged
    );
    fs::write(source.join("nessa"), b"app was replaced").unwrap();
    assert_eq!(
        stage_runtime(&source, &versions, &fingerprint).unwrap(),
        staged
    );
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    fs::write(staged.join("nessa"), b"corruption").unwrap();
    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"corruption");
}

#[test]
fn cloned_files_are_synced_through_the_normalized_finalization_boundary() {
    let fixture = Fixture::new();
    let source = fixture.source().join("node");
    set_test_attribute(&source);
    let cloned = fixture.0.join("cloned-node");
    assert!(clone_file(&source, &cloned).unwrap());
    assert!(has_test_attribute(&cloned));

    let file = OpenOptions::new().write(true).open(&cloned).unwrap();
    finalize_file(&file, 0o755).unwrap();

    assert!(!has_test_attribute(&cloned));
    assert_eq!(
        fs::metadata(&cloned).unwrap().permissions().mode() & 0o777,
        0o711
    );
}

#[test]
fn published_runtime_removes_bundle_extended_attributes() {
    let fixture = Fixture::new();
    let source = fixture.source();
    set_test_attribute(&source.join("nessa"));
    set_test_attribute(&source.join("node"));
    let fingerprint = Fixture::manifest(&source);

    let staged = stage_runtime(&source, &fixture.0.join("versions"), &fingerprint).unwrap();

    assert!(has_test_attribute(&source.join("nessa")));
    assert!(has_test_attribute(&source.join("node")));
    assert!(!has_test_attribute(&staged.join("nessa")));
    assert!(!has_test_attribute(&staged.join("node")));
}

#[test]
fn byte_copy_fallback_preserves_the_published_runtime_contract() {
    let fixture = Fixture::new();
    let source = fixture.source();
    set_test_attribute(&source.join("nessa"));
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");

    let staged =
        stage_runtime_using(&source, &versions, &fingerprint, force_byte_copy, None).unwrap();

    assert_eq!(staged, versions.join(&fingerprint));
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    assert_eq!(
        fs::metadata(staged.join("nessa"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o711
    );
    assert!(has_test_attribute(&source.join("nessa")));
    assert!(!has_test_attribute(&staged.join("nessa")));
    assert_eq!(tree_fingerprint(&staged).unwrap(), fingerprint);
}

#[test]
fn clone_attempt_failure_removes_its_unpublished_runtime() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");

    let error = stage_runtime_using(&source, &versions, &fingerprint, fail_clone_attempt, None)
        .unwrap_err();

    assert_eq!(error, "forced clone failure");
    assert!(!versions.join(&fingerprint).exists());
    assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
}
#[test]
fn mixed_or_interrupted_copy_never_publishes_or_removes_another_attempt() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    nessa_local_storage::create_directory(&versions).unwrap();
    let unrelated = versions.join(".staging-another-attempt");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("keep"), b"keep").unwrap();
    fs::write(source.join("node"), b"mixed new version").unwrap();
    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert!(!versions.join(&fingerprint).exists());
    assert_eq!(fs::read_dir(&versions).unwrap().count(), 1);
    assert!(unrelated.join("keep").exists());
}
#[test]
fn links_special_entries_and_non_utf8_names_are_rejected() {
    for case in ["absolute", "escape", "broken", "fifo", "manifest-link"] {
        let fixture = Fixture::new();
        let source = fixture.source();
        let fingerprint = Fixture::manifest(&source);
        let external = fixture.0.join("outside");
        fs::write(&external, b"outside").unwrap();
        match case {
            "absolute" => symlink(source.join("node"), source.join("bad")).unwrap(),
            "escape" => symlink("../outside", source.join("bad")).unwrap(),
            "broken" => symlink("missing", source.join("bad")).unwrap(),
            "fifo" => {
                let path = CString::new(source.join("bad").to_str().unwrap()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            "manifest-link" => {
                fs::rename(source.join("manifest.json"), source.join("saved-manifest")).unwrap();
                symlink("saved-manifest", source.join("manifest.json")).unwrap();
            }
            _ => unreachable!(),
        }
        let versions = fixture.0.join("versions");
        assert!(
            stage_runtime(&source, &versions, &fingerprint).is_err(),
            "{case}"
        );
        assert!(!versions.join(&fingerprint).exists());
        assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
    }
}
#[test]
fn published_permissions_and_manifest_are_validated_without_repair() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let staged = stage_runtime(&source, &versions, &fingerprint).unwrap();
    fs::set_permissions(staged.join("node"), Permissions::from_mode(0o644)).unwrap();
    assert!(validate_runtime(&staged, &fingerprint).is_err());
    assert_eq!(
        fs::metadata(staged.join("node"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    fs::set_permissions(staged.join("node"), Permissions::from_mode(0o600)).unwrap();
    fs::write(staged.join("manifest.json"), b"invalid").unwrap();
    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert!(
        staged.exists(),
        "a failed validation removed the published generation"
    );
    assert_eq!(fs::read(staged.join("manifest.json")).unwrap(), b"invalid");
}

#[test]
fn validation_cache_reuses_only_the_same_private_published_directory_identity() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let validated = ValidatedRuntimes::default();

    let staged = stage_runtime_cached(&source, &versions, &fingerprint, &validated).unwrap();
    assert!(validated.contains(&staged, &fingerprint).unwrap());
    assert_eq!(
        stage_runtime_cached(&source, &versions, &fingerprint, &validated).unwrap(),
        staged
    );
    assert!(!validated.contains(&staged, &"b".repeat(64)).unwrap());

    fs::remove_dir_all(&staged).unwrap();
    nessa_local_storage::create_directory(&staged).unwrap();
    assert!(!validated.contains(&staged, &fingerprint).unwrap());
    assert!(stage_runtime_cached(&source, &versions, &fingerprint, &validated).is_err());
}

#[test]
fn non_utf8_representation_is_rejected_even_on_filesystems_that_cannot_create_it() {
    let invalid = OsString::from_vec(vec![0xff]);
    assert!(entry_name(invalid.clone()).is_err());
    assert!(utf8(Path::new(&invalid)).is_err());
}
#[test]
fn retained_versions_and_exclusive_publication_never_replace_existing_directories() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let first = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let original = stage_runtime(&source, &versions, &first).unwrap();
    fs::write(source.join("node"), b"new node").unwrap();
    let second = Fixture::manifest(&source);
    let updated = stage_runtime(&source, &versions, &second).unwrap();
    assert_ne!(original, updated);
    assert_eq!(fs::read(original.join("node")).unwrap(), b"node");
    assert_eq!(fs::read(updated.join("node")).unwrap(), b"new node");
    let temporary = versions.join(".owned-test");
    fs::create_dir(&temporary).unwrap();
    fs::write(temporary.join("keep"), b"keep").unwrap();
    let reserved = versions.join("reserved");
    fs::create_dir(&reserved).unwrap();
    assert!(publish(&temporary, &reserved).is_err());
    assert!(temporary.join("keep").exists());
    assert_eq!(fs::read_dir(reserved).unwrap().count(), 0);
}

/// The running job keeps the inode it exec'd. Later opens by the bundle path
/// see a replacement, or fail if that path was moved. Nothing here retargets
/// launchd onto the staged copy mid-session.
#[test]
fn the_first_launch_keeps_the_bundle_path_when_the_app_is_replaced_or_moved() {
    let bundled = Path::new("/Applications/Nessa.app/Contents/Resources/runtime");
    let planned = Path::new(
        "/Users/me/Library/Application Support/Nessa/gateway-runtimes/so.nessa.gateway.prod/ab",
    );
    let launched = super::super::registration_runtime(false, false, bundled, planned);
    assert_eq!(launched, bundled);
    assert_eq!(launch_settings(launched), launch_settings(bundled));
    assert!(!launch_settings(launched)
        .to_string()
        .contains(planned.to_str().unwrap()));
    // The same process keeps the bundle path after publication. The next
    // launch of the app is what points launchd at the published tree.
    assert_eq!(
        super::super::registration_runtime(true, true, bundled, planned),
        bundled
    );
    assert_eq!(
        super::super::registration_runtime(true, false, bundled, planned),
        planned
    );
}

#[test]
fn moving_the_bundle_during_copy_removes_only_this_attempt() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    nessa_local_storage::create_directory(&versions).unwrap();
    let unrelated = versions.join(".staging-another-attempt");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("keep"), b"keep").unwrap();
    let source_path = source.clone();
    let moved = fixture.0.join("moved-bundle");
    let moved_during_copy = moved.clone();
    let _hook = CopyHookGuard::set(move |copied| {
        if copied.file_name().and_then(|name| name.to_str()) == Some("manifest.json") {
            fs::rename(&source_path, &moved_during_copy).unwrap();
            return Err("bundle moved during copy".into());
        }
        Ok(())
    });

    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert!(!versions.join(&fingerprint).exists());
    assert!(!source.exists());
    assert_eq!(fs::read(moved.join("nessa")).unwrap(), b"gateway\0bytes");
    assert_eq!(fs::read(moved.join("node")).unwrap(), b"node");
    assert_eq!(fs::read_dir(&versions).unwrap().count(), 1);
    assert_eq!(fs::read(unrelated.join("keep")).unwrap(), b"keep");
}

#[test]
fn replacing_an_uncopied_bundle_file_during_copy_fails_closed() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let node = source.join("node");
    let _hook = CopyHookGuard::set(move |copied| {
        if copied.file_name().and_then(|name| name.to_str()) == Some("manifest.json") {
            fs::write(&node, b"replaced while the gateway is running").unwrap();
            return Err("bundle file replaced during copy".into());
        }
        Ok(())
    });

    assert!(stage_runtime(&source, &versions, &fingerprint).is_err());
    assert!(!versions.join(&fingerprint).exists());
    assert_eq!(fs::read_dir(&versions).unwrap().count(), 0);
    assert_eq!(
        fs::read(source.join("node")).unwrap(),
        b"replaced while the gateway is running"
    );
    assert_eq!(fs::read(source.join("nessa")).unwrap(), b"gateway\0bytes");
}

#[test]
fn file_copies_overlap_when_the_machine_has_more_than_one_worker() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let in_flight = AtomicUsize::new(0);
    let max_in_flight = AtomicUsize::new(0);
    stage_runtime_using(
        &source,
        &versions,
        &fingerprint,
        |path, output| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_in_flight.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(50));
            let result = force_byte_copy(path, output);
            in_flight.fetch_sub(1, Ordering::SeqCst);
            result
        },
        None,
    )
    .unwrap();
    let max = max_in_flight.load(Ordering::SeqCst);
    if parallelism() > 1 {
        assert!(max > 1, "file copies stayed serial: max in flight {max}");
    } else {
        assert_eq!(max, 1);
    }
}

#[test]
fn sync_progress_counts_a_file_only_after_its_sync_returns() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    nessa_local_storage::create_directory(&source).unwrap();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let events = Arc::new(Mutex::new(Vec::<RuntimeCopyProgress>::new()));
    let during_sync = events.clone();
    let _hook = SyncHookGuard::set(move || {
        let events = during_sync.lock().expect("progress");
        assert!(
            events.iter().all(|event| event.files_completed == 0),
            "progress counted a file before sync_all: {events:?}"
        );
        assert!(
            events
                .last()
                .is_some_and(|event| event.files_total == 1 && event.bytes_total > 0),
            "the copy total is known before sync_all: {events:?}"
        );
    });
    let recorded = events.clone();
    stage_runtime_using(
        &source,
        &versions,
        &fingerprint,
        clone_file,
        Some(&|progress| recorded.lock().expect("progress").push(progress)),
    )
    .unwrap();
    let events = events.lock().expect("progress");
    let last = events.last().expect("progress after sync");
    assert_eq!(last.state, RuntimeCopyPhase::Copying);
    assert_eq!(last.files_completed, 1);
    assert_eq!(last.files_total, 1);
    assert_eq!(last.bytes_completed, last.bytes_total);
    assert!(last.bytes_total > 0);
}

/// Two sync workers can finish together. The first report holds the count
/// until it returns, so the other file cannot publish a lower percent, or the
/// final count, while that report is still running.
#[test]
fn a_slower_progress_report_does_not_move_the_percent_backwards() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    nessa_local_storage::create_directory(&source).unwrap();
    fs::write(source.join("large-a"), vec![b'a'; 4096]).unwrap();
    fs::write(source.join("large-b"), vec![b'b'; 4096]).unwrap();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let parallel = parallelism() > 1;
    let hold = Arc::new(AtomicBool::new(parallel));
    let inside = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::<RuntimeCopyProgress>::new()));
    let watcher = parallel.then(|| {
        let events = events.clone();
        let inside = inside.clone();
        let hold = hold.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !inside.load(Ordering::SeqCst) {
                assert!(
                    Instant::now() < deadline,
                    "no partial progress report started"
                );
                thread::yield_now();
            }
            thread::sleep(Duration::from_millis(40));
            let events = events.lock().expect("progress");
            let partials = events
                .iter()
                .filter(|event| {
                    event.files_completed > 0 && event.files_completed < event.files_total
                })
                .count();
            let finished = events
                .iter()
                .filter(|event| event.files_total > 0 && event.files_completed == event.files_total)
                .count();
            assert_eq!(
                partials, 1,
                "another report ran while the first callback held the count: {events:?}"
            );
            assert_eq!(
                finished, 0,
                "the final report landed while an earlier callback was still running: {events:?}"
            );
            hold.store(false, Ordering::SeqCst);
        })
    });
    let recorded = events.clone();
    let hold_for_report = hold.clone();
    let inside_for_report = inside.clone();
    stage_runtime_using(
        &source,
        &versions,
        &fingerprint,
        clone_file,
        Some(&|progress| {
            recorded.lock().expect("progress").push(progress);
            if parallel
                && progress.files_completed > 0
                && progress.files_completed < progress.files_total
            {
                inside_for_report.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(3);
                while hold_for_report.load(Ordering::SeqCst) {
                    assert!(
                        Instant::now() < deadline,
                        "the following sync did not stay behind this report"
                    );
                    thread::yield_now();
                }
            }
        }),
    )
    .unwrap();
    if let Some(watcher) = watcher {
        watcher.join().expect("progress watcher");
    }
    let events = events.lock().expect("progress");
    let mut previous = 0u64;
    for event in events.iter() {
        let percent = if event.bytes_total == 0 {
            0
        } else {
            event.bytes_completed.saturating_mul(100) / event.bytes_total
        };
        assert!(percent >= previous, "progress moved backwards: {events:?}");
        previous = percent;
    }
    let last = events.last().expect("final progress");
    assert_eq!(last.files_completed, last.files_total);
    assert_eq!(last.bytes_completed, last.bytes_total);
    assert_eq!(
        events
            .iter()
            .position(|event| event.files_completed == event.files_total),
        Some(events.len() - 1)
    );
}

#[test]
fn a_reused_generation_reports_no_copy_progress() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    stage_runtime(&source, &versions, &fingerprint).unwrap();
    let reports = AtomicUsize::new(0);
    stage_runtime_using(
        &source,
        &versions,
        &fingerprint,
        clone_file,
        Some(&|_| {
            reports.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    assert_eq!(reports.load(Ordering::SeqCst), 0);
}

#[test]
fn replacing_bundle_after_manifest_selection_cannot_publish_the_wrong_version() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let selected = Fixture::manifest(&source);
    fs::rename(&source, fixture.0.join("previous-bundle")).unwrap();
    let replacement = fixture.source();
    fs::write(replacement.join("nessa"), b"replacement gateway").unwrap();
    assert_ne!(Fixture::manifest(&replacement), selected);
    let versions = fixture.0.join("versions");
    assert!(stage_runtime(&replacement, &versions, &selected).is_err());
    assert!(!versions.join(selected).exists());
    assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
}

struct ArmedMeasurement {
    finished: bool,
}

impl ArmedMeasurement {
    fn begin() -> Self {
        begin_stage_measurement();
        Self { finished: false }
    }

    fn end(&mut self) -> StagePhases {
        self.finished = true;
        end_stage_measurement()
    }
}

impl Drop for ArmedMeasurement {
    fn drop(&mut self) {
        if !self.finished {
            let _ = end_stage_measurement();
        }
    }
}

#[test]
fn measurement_ledger_counts_the_staged_files_and_records_sync_and_digest() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);
    let mut armed = ArmedMeasurement::begin();
    stage_runtime(&source, &fixture.0.join("versions"), &fingerprint).unwrap();
    let phases = armed.end();
    assert_eq!(phases.files, 3);
    assert_eq!(phases.cloned_files + phases.copied_files, 3);
    assert!(phases.directories >= 1);
    assert!(phases.file_sync > Duration::ZERO);
    assert!(phases.xattr > Duration::ZERO);
    assert!(phases.digest > Duration::ZERO);
    assert!(phases.publish > Duration::ZERO);
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1_000.0
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn command_text(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn set_quarantine(path: &Path) {
    let c_path = CString::new(path.to_str().unwrap()).unwrap();
    let name = CString::new("com.apple.quarantine").unwrap();
    let value = b"0083;00000000;;";
    assert_eq!(
        unsafe {
            libc::setxattr(
                c_path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        },
        0,
        "setxattr {}",
        path.display()
    );
}

fn quarantine_tree(dir: &Path) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            quarantine_tree(&path);
        } else if kind.is_file() {
            set_quarantine(&path);
        }
    }
}

fn write_repeated(path: &Path, len: u64) {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(path)
        .unwrap();
    let chunk = [0xA5u8; 1024 * 1024];
    let mut left = len;
    while left > 0 {
        let n = usize::try_from(left.min(chunk.len() as u64)).unwrap();
        file.write_all(&chunk[..n]).unwrap();
        left -= u64::try_from(n).unwrap();
    }
    file.sync_all().unwrap();
}

struct MeasuredRoot(PathBuf);

impl Drop for MeasuredRoot {
    fn drop(&mut self) {
        if self.0.components().any(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .starts_with("nessa-stage-measure-")
        }) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn fresh_application_support() -> MeasuredRoot {
    let home = std::env::var("HOME").expect("HOME");
    let root = PathBuf::from(home)
        .join("Library/Application Support")
        .join(format!(
            "nessa-stage-measure-{}",
            random_generation().unwrap()
        ));
    fs::create_dir_all(&root).unwrap();
    MeasuredRoot(root)
}

struct ExecResult {
    ms: f64,
    status: Option<i32>,
    error: Option<String>,
}

fn timed_exec(program: &Path, args: &[&str], timeout: Duration) -> ExecResult {
    let started = Instant::now();
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return ExecResult {
                ms: millis(started.elapsed()),
                status: None,
                error: Some(error.to_string()),
            };
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let error = (!status.success()).then(|| {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
                    }
                    format!("exit {status}: {}", stderr.trim())
                });
                return ExecResult {
                    ms: millis(started.elapsed()),
                    status: status.code(),
                    error,
                };
            }
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return ExecResult {
                    ms: millis(started.elapsed()),
                    status: None,
                    error: Some(format!("exceeded {}s", timeout.as_secs())),
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                return ExecResult {
                    ms: millis(started.elapsed()),
                    status: None,
                    error: Some(error.to_string()),
                };
            }
        }
    }
}

fn sync_microbench(dir: &Path) -> serde_json::Value {
    let count = 128u64;
    for index in 0..count {
        let path = dir.join(format!("sync-{index}"));
        fs::write(&path, [0x5Au8; 4096]).unwrap();
    }
    let mut paths = Vec::new();
    for index in 0..count {
        paths.push(dir.join(format!("sync-{index}")));
    }
    let time_all = |sync: fn(&fs::File) -> Result<(), String>| -> serde_json::Value {
        let started = Instant::now();
        for path in &paths {
            let file = fs::File::open(path).unwrap();
            if let Err(error) = sync(&file) {
                return serde_json::json!({
                    "ms": millis(started.elapsed()),
                    "error": error,
                });
            }
        }
        serde_json::json!(millis(started.elapsed()))
    };
    serde_json::json!({
        "files": count,
        "file_sync_all_ms": time_all(|file| file.sync_all().map_err(|error| error.to_string())),
        "libc_fsync_ms": time_all(|file| {
            if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            }
        }),
        "f_fullfsync_ms": time_all(|file| {
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            }
        }),
    })
}

/// Where a cold `stage_runtime` spends its time, plus launchd bootstrap and
/// first execution beside it.
///
/// The tree matches the first-launch shape from #109: thousands of files and
/// about 390MB, with a quarantine attribute on every file. The destination is
/// a directory created for this attempt under Application Support. Set
/// `NESSA_STAGING_PURGE=required` to drop the disk cache after the tree is
/// built and before staging. `NESSA_STAGING_MEASUREMENT_NODE` names a real
/// Node binary to stage and execute; without it the executable is `/usr/bin/true`.
#[test]
#[ignore = "cold ~390MB stage_runtime measurement for a macOS runner"]
fn measure_runtime_staging_phases() {
    let files = env_u64("NESSA_STAGING_FILES", 6337);
    let directories = env_u64("NESSA_STAGING_DIRS", 685);
    let bytes = env_u64("NESSA_STAGING_BYTES", 393_574_698);
    let purge_required = std::env::var("NESSA_STAGING_PURGE").ok().as_deref() == Some("required");
    let node = std::env::var_os("NESSA_STAGING_MEASUREMENT_NODE").map(PathBuf::from);

    let root = fresh_application_support();
    let source = root.0.join("source");
    fs::create_dir_all(&source).unwrap();
    let subdirs = directories.saturating_sub(1);
    for index in 0..subdirs {
        fs::create_dir(source.join(format!("d{index:04}"))).unwrap();
    }
    let executable_name = if node.is_some() { "node" } else { "probe" };
    let executable_src = node.unwrap_or_else(|| PathBuf::from("/usr/bin/true"));
    let executable_len = fs::copy(&executable_src, source.join(executable_name)).unwrap();
    fs::set_permissions(source.join(executable_name), Permissions::from_mode(0o755)).unwrap();
    let small_count = files.saturating_sub(3);
    let small_bytes = small_count.saturating_mul(64);
    for index in 0..small_count {
        let dir = index % subdirs.max(1);
        let path = if subdirs == 0 {
            source.join(format!("f{index:05}"))
        } else {
            source.join(format!("d{dir:04}/f{index:05}"))
        };
        fs::write(&path, [0x11u8; 64]).unwrap();
    }
    let payload = bytes.saturating_sub(executable_len + small_bytes);
    if payload > 0 {
        write_repeated(&source.join("payload.bin"), payload);
    }
    if payload > 0 {
        for index in 0..8 {
            let from = if subdirs == 0 {
                source.join(format!("link-{index}"))
            } else {
                source.join(format!("d0000/link-{index}"))
            };
            let target = if subdirs == 0 {
                "payload.bin"
            } else {
                "../payload.bin"
            };
            symlink(target, &from).unwrap();
        }
    }
    let fingerprint = Fixture::manifest(&source);
    quarantine_tree(&source);

    let purge = if purge_required {
        let output = Command::new("sudo").args(["-n", "purge"]).output();
        match output {
            Ok(output) if output.status.success() => (true, String::new()),
            Ok(output) => (
                false,
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ),
            Err(error) => (false, error.to_string()),
        }
    } else {
        (false, "not requested".to_owned())
    };
    assert!(
        !purge_required || purge.0,
        "disk cache purge failed: {}",
        purge.1
    );

    let installations = root.0.join("versions");
    let mut armed = ArmedMeasurement::begin();
    let started = Instant::now();
    let staged = stage_runtime(&source, &installations, &fingerprint);
    let wall = started.elapsed();
    let phases = armed.end();
    let staged = staged.unwrap_or_else(|error| panic!("stage_runtime failed: {error}"));

    let exec_args: &[&str] = if executable_name == "node" {
        &["--version"]
    } else {
        &[]
    };
    let first_exec = timed_exec(
        &staged.join(executable_name),
        exec_args,
        Duration::from_secs(180),
    );
    let second_exec = timed_exec(
        &staged.join(executable_name),
        exec_args,
        Duration::from_secs(180),
    );

    let quarantined = root.0.join("quarantined-exec");
    fs::copy(&executable_src, &quarantined).unwrap();
    fs::set_permissions(&quarantined, Permissions::from_mode(0o755)).unwrap();
    set_quarantine(&quarantined);
    let quarantined_first = timed_exec(&quarantined, exec_args, Duration::from_secs(180));
    let quarantined_second = timed_exec(&quarantined, exec_args, Duration::from_secs(180));

    let label = format!(
        "com.nessa.stage-measure.{}",
        &random_generation().unwrap()[..12]
    );
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/{label}");
    let plist_path = root.0.join("measure.plist");
    fs::write(
        &plist_path,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array><string>/bin/sleep</string><string>30</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>"#
        ),
    )
    .unwrap();
    struct Bootout(String);
    impl Drop for Bootout {
        fn drop(&mut self) {
            let _ = Command::new("/bin/launchctl")
                .args(["bootout", &self.0])
                .output();
        }
    }
    let _bootout = Bootout(service.clone());
    let launch_started = Instant::now();
    let launch = Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(&plist_path)
        .output()
        .unwrap();
    let launch_ms = millis(launch_started.elapsed());
    let launch_error = (!launch.status.success())
        .then(|| String::from_utf8_lossy(&launch.stderr).trim().to_owned());

    let bench_dir = root.0.join("sync-bench");
    fs::create_dir(&bench_dir).unwrap();
    let sync = sync_microbench(&bench_dir);
    let accounted = phases.open
        + phases.clone
        + phases.byte_copy
        + phases.permissions
        + phases.xattr
        + phases.file_sync
        + phases.dir_sync
        + phases.private_walk
        + phases.manifest
        + phases.digest
        + phases.publish
        + phases.parent_sync;
    let report = serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "workers": parallelism(),
        "destination": "fresh directory under ~/Library/Application Support",
        "destination_path": root.0,
        "filesystem": command_text("stat", &["-f", "%T", root.0.to_str().unwrap()]),
        "product_version": command_text("sw_vers", &["-productVersion"]),
        "architecture": command_text("uname", &["-m"]),
        "cache_purge": { "requested": purge_required, "succeeded": purge.0, "detail": purge.1 },
        "source": {
            "files": phases.files,
            "directories": phases.directories,
            "symlinks": phases.symlinks,
            "bytes": phases.bytes,
            "cloned_files": phases.cloned_files,
            "copied_files": phases.copied_files,
            "executable": executable_name,
        },
        "phases_ms": {
            "open": millis(phases.open),
            "clone": millis(phases.clone),
            "byte_copy": millis(phases.byte_copy),
            "permissions": millis(phases.permissions),
            "xattr": millis(phases.xattr),
            "file_sync": millis(phases.file_sync),
            "file_sync_work": millis(phases.file_sync_work),
            "dir_sync": millis(phases.dir_sync),
            "private_walk": millis(phases.private_walk),
            "manifest": millis(phases.manifest),
            "digest": millis(phases.digest),
            "publish": millis(phases.publish),
            "parent_sync": millis(phases.parent_sync),
            "accounted": millis(accounted),
            "wall": millis(wall),
            "unaccounted": millis(wall.saturating_sub(accounted)),
        },
        "launchd_bootstrap_ms": launch_ms,
        "launchd_bootstrap_error": launch_error,
        "launchd_program": "/bin/sleep 30",
        "first_exec_ms": first_exec.ms,
        "first_exec_status": first_exec.status,
        "first_exec_error": first_exec.error,
        "second_exec_ms": second_exec.ms,
        "second_exec_error": second_exec.error,
        "quarantined_first_exec_ms": quarantined_first.ms,
        "quarantined_first_exec_status": quarantined_first.status,
        "quarantined_first_exec_error": quarantined_first.error,
        "quarantined_second_exec_ms": quarantined_second.ms,
        "quarantined_second_exec_error": quarantined_second.error,
        "sync_microbench": sync,
    });
    let line = serde_json::to_string(&report).unwrap();
    println!("NESSA_STAGE_PHASES {line}");
    if let Ok(path) = std::env::var("NESSA_STAGING_MEASUREMENT_OUT") {
        fs::write(path, format!("{line}\n")).unwrap();
    }
    assert!(phases.cloned_files > 0, "APFS clone did not run: {line}");
    assert!(phases.file_sync > Duration::ZERO);
    assert!(phases.digest > Duration::ZERO);
    assert!(
        launch.status.success(),
        "launchctl bootstrap failed: {}",
        String::from_utf8_lossy(&launch.stderr)
    );
}

/// Releases a deferred publication if the test fails while the thread is paused.
struct ReleaseOnDrop(Arc<super::super::StagePause>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn label_lock_is_held(path: &Path) -> bool {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return false;
    }
    let error = std::io::Error::last_os_error();
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::WouldBlock,
        "label lock: {error}"
    );
    true
}

#[test]
fn the_label_lock_is_released_when_nothing_was_deferred() {
    let fixture = Fixture::new();
    let locks = fixture.0.join("locks");
    nessa_local_storage::create_directory(&locks).unwrap();
    let lock_file = locks.join("gateway-upgrade.lock");
    {
        let _held = super::super::HeldRegistration::lock(&locks).unwrap();
        assert!(label_lock_is_held(&lock_file));
    }
    assert!(!label_lock_is_held(&lock_file));
}

#[test]
fn deferred_publication_holds_the_label_lock_until_stage_runtime_finishes() {
    let fixture = Fixture::new();
    let source = fixture.source();
    set_test_attribute(&source.join("nessa"));
    let fingerprint = Fixture::manifest(&source);
    let versions = fixture.0.join("versions");
    let locks = fixture.0.join("locks");
    nessa_local_storage::create_directory(&locks).unwrap();
    let lock_file = locks.join("gateway-upgrade.lock");
    let pause = super::super::StagePause::new();
    let release = ReleaseOnDrop(pause.clone());
    {
        let mut held = super::super::HeldRegistration::lock(&locks).unwrap();
        held.defer_stage_until(source, versions.clone(), fingerprint.clone(), pause.clone());
    }
    pause.wait_until_staging_holds_the_lock();
    assert!(label_lock_is_held(&lock_file));
    assert!(!versions.join(&fingerprint).exists());
    drop(release);
    pause.wait_until_finished().unwrap();
    let staged = versions.join(&fingerprint);
    assert_eq!(
        fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::read(staged.join("nessa")).unwrap(), b"gateway\0bytes");
    assert!(!has_test_attribute(&staged.join("nessa")));
    assert!(!label_lock_is_held(&lock_file));
    let leftovers = fs::read_dir(&versions)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".staging-"))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_failed_deferred_publication_releases_the_lock_and_leaves_no_generation() {
    let fixture = Fixture::new();
    let source = fixture.0.join("empty");
    fs::create_dir(&source).unwrap();
    let fingerprint = "ab".repeat(32);
    let versions = fixture.0.join("versions");
    let locks = fixture.0.join("locks");
    nessa_local_storage::create_directory(&locks).unwrap();
    let lock_file = locks.join("gateway-upgrade.lock");
    let pause = super::super::StagePause::new();
    let release = ReleaseOnDrop(pause.clone());
    {
        let mut held = super::super::HeldRegistration::lock(&locks).unwrap();
        held.defer_stage_until(source, versions.clone(), fingerprint.clone(), pause.clone());
    }
    pause.wait_until_staging_holds_the_lock();
    assert!(label_lock_is_held(&lock_file));
    drop(release);
    assert!(pause.wait_until_finished().is_err());
    assert!(!versions.join(&fingerprint).exists());
    if versions.exists() {
        let leftovers = fs::read_dir(&versions)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with(".staging-"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
    assert!(!label_lock_is_held(&lock_file));
}
