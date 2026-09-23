use super::super::generation::random_generation;
use super::{
    clone_file, copy_directory, entry_name, finalize_file, finish_staging_profile, launch_settings,
    publish, stage_runtime, stage_runtime_using, start_staging_profile, tree_fingerprint, utf8,
    validate_runtime, CloneFile, StagingProfile,
};
use crate::gateway::infrastructure::macos::runtime_fingerprint;
use serde_json::json;
use std::{
    ffi::{CString, OsString},
    fs::{self, OpenOptions, Permissions},
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
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

#[derive(Default)]
struct RuntimeSize {
    files: u64,
    directories: u64,
    links: u64,
    bytes: u64,
}

const MOST_MEASUREMENT_ENTRIES: u64 = 50_000;
const MOST_MEASUREMENT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn enforce_runtime_size_limits(size: &RuntimeSize) -> Result<(), String> {
    let entries = size
        .files
        .checked_add(size.directories)
        .and_then(|entries| entries.checked_add(size.links))
        .ok_or("Runtime entry count overflowed")?;
    if entries > MOST_MEASUREMENT_ENTRIES {
        return Err(format!(
            "Runtime measurement exceeded {MOST_MEASUREMENT_ENTRIES} entries"
        ));
    }
    if size.bytes > MOST_MEASUREMENT_BYTES {
        return Err(format!(
            "Runtime measurement exceeded {MOST_MEASUREMENT_BYTES} bytes"
        ));
    }
    Ok(())
}

fn runtime_size(path: &Path, size: &mut RuntimeSize) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        size.links = size
            .links
            .checked_add(1)
            .ok_or("Runtime link count overflowed")?;
        enforce_runtime_size_limits(size)?;
    } else if metadata.is_dir() {
        size.directories = size
            .directories
            .checked_add(1)
            .ok_or("Runtime directory count overflowed")?;
        enforce_runtime_size_limits(size)?;
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            runtime_size(&entry.map_err(|error| error.to_string())?.path(), size)?;
        }
    } else if metadata.is_file() {
        size.files = size
            .files
            .checked_add(1)
            .ok_or("Runtime file count overflowed")?;
        size.bytes = size
            .bytes
            .checked_add(metadata.len())
            .ok_or("Runtime byte count overflowed")?;
        enforce_runtime_size_limits(size)?;
    } else {
        return Err("Runtime measurement found an unsupported entry".into());
    }
    Ok(())
}

fn measured_copy(
    destination: &Path,
    source: &Path,
    clone: CloneFile,
    profile: bool,
) -> (Duration, Option<StagingProfile>) {
    nessa_local_storage::create_directory(destination).unwrap();
    if profile {
        start_staging_profile();
    }
    let started = Instant::now();
    copy_directory(source, source, destination, clone).unwrap();
    let elapsed = started.elapsed();
    let profile = profile.then(finish_staging_profile);
    (elapsed, profile)
}

fn measurement_provenance() -> Result<(), &'static str> {
    let build_head = option_env!("NESSA_STAGING_BUILD_HEAD").ok_or(
        "build this ignored harness with NESSA_STAGING_BUILD_HEAD set to the checkout commit",
    )?;
    if !matches!(build_head.len(), 40 | 64)
        || !build_head.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("NESSA_STAGING_BUILD_HEAD must be a full Git object ID");
    }
    let build_invocation = option_env!("NESSA_STAGING_BUILD_INVOCATION").ok_or(
        "build this ignored harness with NESSA_STAGING_BUILD_INVOCATION set to the exact Cargo command",
    )?;
    if build_invocation.trim().is_empty() {
        return Err("NESSA_STAGING_BUILD_INVOCATION must not be empty");
    }
    let binary = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .expect("the running test binary path must be available");
    eprintln!(
        "compiled_checkout={:?} compiled_head={} test_binary={} cargo_package_version={} debug_assertions={} build_invocation={:?}",
        env!("CARGO_MANIFEST_DIR"),
        build_head,
        binary.display(),
        env!("CARGO_PKG_VERSION"),
        cfg!(debug_assertions),
        build_invocation,
    );
    Ok(())
}

fn comparison_schedule(repetition: usize) -> ([(&'static str, CloneFile); 2], [bool; 2]) {
    let modes = if repetition % 2 == 0 {
        [
            ("clone", clone_file as CloneFile),
            ("byte-copy", force_byte_copy as CloneFile),
        ]
    } else {
        [
            ("byte-copy", force_byte_copy as CloneFile),
            ("clone", clone_file as CloneFile),
        ]
    };
    let collector_order = if repetition % 2 == 0 {
        [true, false]
    } else {
        [false, true]
    };
    (modes, collector_order)
}

fn assert_success_profile(profile: &StagingProfile) {
    for operation in [
        &profile.clone,
        &profile.byte_copy,
        &profile.permissions,
        &profile.extended_attributes,
        &profile.file_sync,
        &profile.directory_sync,
    ] {
        assert_eq!(operation.errors, 0);
        assert!(operation.max <= operation.sum);
    }
}

#[test]
fn comparison_schedule_gives_each_mode_both_collector_orders() {
    let mut clone_first = Vec::new();
    let mut byte_copy_first = Vec::new();

    for repetition in 0..4 {
        let (modes, collector_order) = comparison_schedule(repetition);
        let names = modes.map(|(name, _)| name);
        assert_eq!(
            names,
            if repetition % 2 == 0 {
                ["clone", "byte-copy"]
            } else {
                ["byte-copy", "clone"]
            }
        );
        for (mode, _) in modes {
            match mode {
                "clone" => clone_first.push(collector_order[0]),
                "byte-copy" => byte_copy_first.push(collector_order[0]),
                _ => unreachable!(),
            }
        }
    }

    assert_eq!(clone_first, [true, false, true, false]);
    assert_eq!(byte_copy_first, [true, false, true, false]);
}

#[test]
fn runtime_inventory_limits_are_enforced_at_the_increment_boundary() {
    let at_entry_limit = RuntimeSize {
        directories: MOST_MEASUREMENT_ENTRIES,
        ..RuntimeSize::default()
    };
    enforce_runtime_size_limits(&at_entry_limit).unwrap();

    let beyond_entry_limit = RuntimeSize {
        directories: MOST_MEASUREMENT_ENTRIES + 1,
        ..RuntimeSize::default()
    };
    assert_eq!(
        enforce_runtime_size_limits(&beyond_entry_limit).unwrap_err(),
        format!("Runtime measurement exceeded {MOST_MEASUREMENT_ENTRIES} entries")
    );

    let beyond_byte_limit = RuntimeSize {
        bytes: MOST_MEASUREMENT_BYTES + 1,
        ..RuntimeSize::default()
    };
    assert_eq!(
        enforce_runtime_size_limits(&beyond_byte_limit).unwrap_err(),
        format!("Runtime measurement exceeded {MOST_MEASUREMENT_BYTES} bytes")
    );
}

/// Measures the real packaged tree without making the routine test suite copy it.
///
/// Run explicitly with `NESSA_STAGING_MEASUREMENT_RUNTIME` naming the runtime
/// resource directory. The source is opened read-only by the staging code; every
/// destination lives below one random temporary directory owned by `Fixture`.
/// Compile it with `NESSA_STAGING_BUILD_HEAD` and
/// `NESSA_STAGING_BUILD_INVOCATION`; both are embedded in the test binary.
#[test]
#[ignore = "copies and validates a packaged runtime supplied by the caller"]
fn measure_packaged_runtime_staging_phases() {
    measurement_provenance().expect("measurement build provenance must be embedded and valid");
    let source = std::env::var_os("NESSA_STAGING_MEASUREMENT_RUNTIME")
        .map(PathBuf::from)
        .expect("NESSA_STAGING_MEASUREMENT_RUNTIME must name a packaged runtime");
    let source = source.canonicalize().expect("runtime source must exist");
    let expected = runtime_fingerprint(&source).expect("runtime manifest must be readable");
    let mut size = RuntimeSize::default();
    runtime_size(&source, &mut size).expect("runtime tree must be measurable");

    let fixture = Fixture::new();
    let phases = fixture.0.join("phases");
    nessa_local_storage::create_directory(&phases).unwrap();
    let temporary = phases.join(".staging-measurement");
    nessa_local_storage::create_directory(&temporary).unwrap();

    start_staging_profile();
    let copy_started = Instant::now();
    copy_directory(&source, &source, &temporary, clone_file).unwrap();
    let copied_in = copy_started.elapsed();
    let copy_profile = finish_staging_profile();

    let validation_started = Instant::now();
    validate_runtime(&temporary, &expected).unwrap();
    let validated_in = validation_started.elapsed();

    let published = phases.join(&expected);
    let publication_started = Instant::now();
    publish(&temporary, &published).unwrap();
    nessa_local_storage::sync_directory(&phases).unwrap();
    let published_in = publication_started.elapsed();

    let installations = fixture.0.join("stage-runtime");
    start_staging_profile();
    let staging_started = Instant::now();
    let staged = stage_runtime(&source, &installations, &expected).unwrap();
    let staged_in = staging_started.elapsed();
    let stage_profile = finish_staging_profile();

    start_staging_profile();
    let reuse_started = Instant::now();
    assert_eq!(
        stage_runtime(&source, &installations, &expected).unwrap(),
        staged
    );
    let reused_in = reuse_started.elapsed();
    let reuse_profile = finish_staging_profile();

    let source_device = fs::metadata(&source).unwrap().dev();
    let destination_device = fs::metadata(&fixture.0).unwrap().dev();

    eprintln!(
        "runtime={} destination_root={} source_device={} destination_device={} same_device={} fingerprint={} files={} directories={} links={} bytes={} copy_sync_attrs_ms={} validation_ms={} publication_ms={} stage_runtime_ms={} reuse_ms={} copy_profile={copy_profile:?} stage_profile={stage_profile:?} reuse_profile={reuse_profile:?}",
        source.display(),
        fixture.0.display(),
        source_device,
        destination_device,
        source_device == destination_device,
        expected,
        size.files,
        size.directories,
        size.links,
        size.bytes,
        copied_in.as_millis(),
        validated_in.as_millis(),
        published_in.as_millis(),
        staged_in.as_millis(),
        reused_in.as_millis(),
    );
}

/// Compares clone and byte-copy paths, including the bounded collector overhead.
///
/// Run explicitly with `NESSA_STAGING_MEASUREMENT_RUNTIME` naming the runtime.
/// `NESSA_STAGING_MEASUREMENT_REPETITIONS` may select one through ten repetitions.
/// Each sample gets a fresh destination, and both mode and collector order alternate.
/// Compile it with `NESSA_STAGING_BUILD_HEAD` and
/// `NESSA_STAGING_BUILD_INVOCATION`; both are embedded in the test binary.
#[test]
#[ignore = "copies a packaged runtime repeatedly for comparative profiling"]
fn compare_packaged_runtime_clone_and_byte_copy_profiles() {
    const DEFAULT_REPETITIONS: usize = 4;
    const MOST_REPETITIONS: usize = 10;

    measurement_provenance().expect("measurement build provenance must be embedded and valid");
    let source = std::env::var_os("NESSA_STAGING_MEASUREMENT_RUNTIME")
        .map(PathBuf::from)
        .expect("NESSA_STAGING_MEASUREMENT_RUNTIME must name a packaged runtime")
        .canonicalize()
        .expect("runtime source must exist");
    let fingerprint = runtime_fingerprint(&source).expect("runtime manifest must be readable");
    let mut size = RuntimeSize::default();
    runtime_size(&source, &mut size).expect("runtime tree must be measurable");
    let repetitions = std::env::var("NESSA_STAGING_MEASUREMENT_REPETITIONS")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .expect("repetitions must be an integer")
        })
        .unwrap_or(DEFAULT_REPETITIONS);
    assert!(
        (1..=MOST_REPETITIONS).contains(&repetitions),
        "repetitions must be between 1 and {MOST_REPETITIONS}"
    );

    let fixture = Fixture::new();
    let source_device = fs::metadata(&source).unwrap().dev();
    let destination_device = fs::metadata(&fixture.0).unwrap().dev();
    eprintln!(
        "runtime={} destination_root={} source_device={} destination_device={} same_device={} fingerprint={} files={} directories={} links={} bytes={} repetitions={}",
        source.display(),
        fixture.0.display(),
        source_device,
        destination_device,
        source_device == destination_device,
        fingerprint,
        size.files,
        size.directories,
        size.links,
        size.bytes,
        repetitions,
    );

    for repetition in 0..repetitions {
        let (modes, collector_order) = comparison_schedule(repetition);
        for (mode_index, (mode, clone)) in modes.into_iter().enumerate() {
            let mut profiled_elapsed = None;
            let mut ordinary_elapsed = None;
            for (sample_index, profile_enabled) in collector_order.into_iter().enumerate() {
                let destination = fixture.0.join(format!(
                    "comparison-{repetition}-{mode_index}-{sample_index}-{mode}"
                ));
                let (elapsed, profile) =
                    measured_copy(&destination, &source, clone, profile_enabled);
                validate_runtime(&destination, &fingerprint).unwrap();
                if let Some(profile) = profile.as_ref() {
                    assert_success_profile(profile);
                    assert_eq!(profile.clone.count, size.files);
                    assert_eq!(profile.cloned_files + profile.clone_fallbacks, size.files);
                    assert!(profile.byte_copy.bytes <= size.bytes);
                    if mode == "byte-copy" {
                        assert_eq!(profile.clone_fallbacks, size.files);
                        assert_eq!(profile.byte_copy.bytes, size.bytes);
                    }
                }
                if profile_enabled {
                    profiled_elapsed = Some(elapsed);
                } else {
                    ordinary_elapsed = Some(elapsed);
                }
                eprintln!(
                    "repetition={} mode={} collector={} order={} elapsed_ms={} profile={profile:?}",
                    repetition + 1,
                    mode,
                    profile_enabled,
                    sample_index + 1,
                    elapsed.as_millis(),
                );
                fs::remove_dir_all(destination).unwrap();
            }
            let profiled_elapsed = profiled_elapsed.unwrap();
            let ordinary_elapsed = ordinary_elapsed.unwrap();
            eprintln!(
                "repetition={} mode={} collector_overhead_ns={} profiled_ns={} ordinary_ns={}",
                repetition + 1,
                mode,
                profiled_elapsed.as_nanos() as i128 - ordinary_elapsed.as_nanos() as i128,
                profiled_elapsed.as_nanos(),
                ordinary_elapsed.as_nanos(),
            );
        }
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

    let staged = stage_runtime_using(&source, &versions, &fingerprint, force_byte_copy).unwrap();

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

    let error =
        stage_runtime_using(&source, &versions, &fingerprint, fail_clone_attempt).unwrap_err();

    assert_eq!(error, "forced clone failure");
    assert!(!versions.join(&fingerprint).exists());
    assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
}

#[test]
fn profiling_guard_counts_an_operation_that_returns_an_error() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let fingerprint = Fixture::manifest(&source);

    start_staging_profile();
    let error = stage_runtime_using(
        &source,
        &fixture.0.join("versions"),
        &fingerprint,
        fail_clone_attempt,
    )
    .unwrap_err();
    let profile = finish_staging_profile();

    assert_eq!(error, "forced clone failure");
    assert_eq!(profile.clone.count, 1);
    assert_eq!(profile.clone.errors, 1);
    assert_eq!(profile.cloned_files, 0);
    assert_eq!(profile.clone_fallbacks, 0);
}

#[test]
fn profiling_records_leaf_operations_and_actual_byte_copy_count() {
    let fixture = Fixture::new();
    let source = fixture.source();
    Fixture::manifest(&source);
    let destination = fixture.0.join("profiled-copy");
    let mut size = RuntimeSize::default();
    runtime_size(&source, &mut size).unwrap();

    let (_, profile) = measured_copy(&destination, &source, force_byte_copy, true);
    let profile = profile.unwrap();

    assert_success_profile(&profile);
    assert_eq!(profile.clone.count, size.files);
    assert_eq!(profile.clone.errors, 0);
    assert_eq!(profile.cloned_files, 0);
    assert_eq!(profile.clone_fallbacks, size.files);
    assert_eq!(profile.byte_copy.count, size.files);
    assert_eq!(profile.byte_copy.bytes, size.bytes);
    assert_eq!(profile.permissions.count, size.files);
    assert_eq!(profile.extended_attributes.count, size.files);
    assert_eq!(profile.file_sync.count, size.files);
    assert_eq!(profile.directory_sync.count, size.directories);
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
