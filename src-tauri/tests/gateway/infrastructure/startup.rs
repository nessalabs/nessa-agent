use super::{diagnose, log_tail, parse_last_exit, LastExit};
use nessa_local_storage::OpenMode;
use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

const READINESS: &str = "the gateway did not advertise the expected runtime identity";
const PORT: u16 = 7420;

/// The exact line the reported v0.1.0 failure left in `~/.nessa/logs/gateway.log`.
const REGISTRY_LOG: &str =
    "ERROR nessa_server::core::error: nessa failed self=authentication setup failed: credential registry is invalid";

fn scratch(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "nessa-startup-{}-{name}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&directory);
    nessa_local_storage::create_directory(&directory).unwrap();
    directory
}

/// The gateway log is reserved privately before launchd opens it, and a log
/// this process cannot vouch for that way is not one it reads.
fn write_private_log(path: &Path, contents: &str) {
    let _ = fs::remove_file(path);
    let mut file = nessa_local_storage::open(path, OpenMode::CreateNew).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
}

#[test]
fn launchd_exit_lines_are_read_across_the_forms_macos_has_printed() {
    assert_eq!(
        parse_last_exit("gui/501/so.nessa.gateway.prod = {\n\tlast exit code = 1\n}"),
        LastExit::Code(1)
    );
    assert_eq!(
        parse_last_exit("\tlast exit code = (never exited)"),
        LastExit::NeverExited
    );
    assert_eq!(parse_last_exit("\tlast exit code = 0"), LastExit::Code(0));
    // Older releases print the wait(2) encoding, whose high byte is the code.
    assert_eq!(
        parse_last_exit("\tlast exit status = 256"),
        LastExit::Code(1)
    );
    assert_eq!(parse_last_exit("\tlast exit status = 0"), LastExit::Code(0));
    assert_eq!(
        parse_last_exit("\tlast exit reason = JETSAM_REASON_MEMORY_IDLE_EXIT"),
        LastExit::Reason("JETSAM_REASON_MEMORY_IDLE_EXIT".into())
    );
}

#[test]
fn an_output_we_cannot_read_reports_nothing_rather_than_death() {
    // Nothing here accelerates a failure: `Unknown` is what keeps the caller
    // waiting for the deadline it was given.
    for text in [
        "gui/501/so.nessa.gateway.prod = {\n\tstate = running\n}",
        "\tlast exit code = not a number",
        "\tlast exit code = ",
        // Two answers in one output is no answer.
        "\tlast exit code = 1\n\tlast exit code = 0",
        "\tlast exit code = 1\n\tlast exit reason = JETSAM_REASON_MEMORY_IDLE_EXIT",
    ] {
        assert_eq!(parse_last_exit(text), LastExit::Unknown, "{text}");
        assert!(!parse_last_exit(text).is_failure(), "{text}");
    }
    // A repeated, agreeing answer is still that answer.
    assert_eq!(
        parse_last_exit("\tlast exit code = 1\n\tlast exit code = 1"),
        LastExit::Code(1)
    );
}

#[test]
fn only_an_unsuccessful_exit_counts_as_one() {
    assert!(LastExit::Code(1).is_failure());
    assert!(LastExit::Code(127).is_failure());
    assert!(LastExit::Reason("JETSAM_REASON_MEMORY_IDLE_EXIT".into()).is_failure());
    assert!(!LastExit::Code(0).is_failure());
    assert!(!LastExit::NeverExited.is_failure());
    assert!(!LastExit::Unknown.is_failure());
}

#[test]
fn the_registry_case_reads_as_a_registry_problem() {
    let failure = diagnose(&LastExit::Code(1), REGISTRY_LOG, PORT, READINESS);
    assert_eq!(
        failure.sentence,
        "Nessa's background service is not starting: its credential registry is not one this version of Nessa can read."
    );
    // The readiness contract's own language is the thing this replaces.
    assert!(!failure.sentence.contains("runtime identity"));
    assert!(!failure.sentence.contains("forward recovery"));
    // And the evidence it was inferred from survives, for the app's log.
    assert!(failure.detail.contains("exit code 1"));
    assert!(failure.detail.contains(READINESS));
    assert!(failure.detail.contains(REGISTRY_LOG));

    // The rest of that error family is still the registry, and is not told it
    // is a version problem when the registry said something else.
    let locked = diagnose(
        &LastExit::Code(1),
        "ERROR nessa failed self=authentication setup failed: credential registry is locked",
        PORT,
        READINESS,
    );
    assert_eq!(
        locked.sentence,
        "Nessa's background service is not starting: its credential registry could not be read."
    );
}

#[test]
fn a_taken_port_names_the_port_the_service_was_registered_for() {
    let failure = diagnose(
        &LastExit::Code(1),
        "ERROR nessa failed: port already in use at 127.0.0.1:7420; stop the other process or set NESSA_PORT",
        PORT,
        READINESS,
    );
    assert_eq!(
        failure.sentence,
        "Nessa's background service is not starting: port 7420 is already in use."
    );
}

#[test]
fn a_program_that_never_ran_is_not_reported_as_something_it_said() {
    // launchd spawn failures and the loader's own refusals both happen before
    // any of the server's code, so the exit status carries them alone.
    for (exit, tail) in [
        (LastExit::Code(127), ""),
        (LastExit::Code(126), ""),
        (
            LastExit::Code(1),
            "dyld[4213]: Library not loaded: @rpath/libnessa.dylib",
        ),
        (LastExit::Code(1), "nessa: No such file or directory"),
    ] {
        assert_eq!(
            diagnose(&exit, tail, PORT, READINESS).sentence,
            "Nessa's background service is not starting: its background program could not be launched."
        );
    }
    // A server that ran far enough to report its own failure is not one that
    // could not be launched, whatever file it went looking for.
    assert_eq!(
        diagnose(
            &LastExit::Code(1),
            "ERROR nessa failed self=invalid configuration: No such file or directory",
            PORT,
            READINESS,
        )
        .sentence,
        "Nessa's background service is not starting."
    );
}

#[test]
fn a_failure_we_cannot_name_still_says_less_than_the_readiness_contract_did() {
    let silent = diagnose(&LastExit::Code(9), "", PORT, READINESS);
    assert_eq!(
        silent.sentence,
        "Nessa's background service is not starting, and it exited without reporting why."
    );
    assert!(silent.detail.contains("none recognised"));
    assert!(!silent.detail.contains("log tail"));

    // An unrecognised message is not shown, but it is what the app logs.
    let spoke = diagnose(
        &LastExit::Code(3),
        "thread 'main' panicked at src/x.rs",
        PORT,
        READINESS,
    );
    assert_eq!(
        spoke.sentence,
        "Nessa's background service is not starting."
    );
    assert!(spoke.detail.contains("thread 'main' panicked at src/x.rs"));
}

#[test]
fn the_log_tail_is_the_end_of_the_log_and_never_the_whole_of_it() {
    let directory = scratch("tail");
    let path = directory.join("gateway.log");
    let mut written = String::new();
    for line in 0..200 {
        written.push_str(&format!("line {line}\n"));
    }
    // A log that has run for weeks is not read whole, and the partial line the
    // seek lands in is dropped rather than reported as something the log said.
    written.insert_str(0, &"x".repeat(32_768));
    written.push('\n');
    write_private_log(&path, &written);
    let tail = log_tail(&path);
    assert!(!tail.contains("xxxx"));
    assert!(tail.lines().count() <= 12);
    assert!(tail.contains("line 199"));
    assert!(!tail.contains("line 100"));

    write_private_log(&path, "only line\n");
    assert_eq!(log_tail(&path), "only line");

    write_private_log(&path, "");
    assert_eq!(log_tail(&path), "");

    // A log we cannot open is not a diagnosis, and not a second failure either.
    assert_eq!(log_tail(&directory.join("absent.log")), "");
    let shared = directory.join("shared.log");
    write_private_log(&shared, "line\nsecond line\n");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(log_tail(&shared), "");
    fs::remove_dir_all(&directory).unwrap();
}
