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

/// Every string below was taken from `launchctl print` on macOS 26, against
/// agents bootstrapped to produce each outcome, rather than written to match
/// the parser.
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
    // launchd annotates the sysexits values and leaves the rest bare. `exit 78`
    // and a plist naming a program that is not there both come back this way.
    assert_eq!(
        parse_last_exit("\tlast exit code = 78: EX_CONFIG"),
        LastExit::Code(78)
    );
    assert_eq!(
        parse_last_exit("\tlast exit code = 64: EX_USAGE"),
        LastExit::Code(64)
    );
    assert_eq!(
        parse_last_exit("\tlast exit code = 127"),
        LastExit::Code(127)
    );
    // A signalled process has no exit code line at all.
    assert_eq!(
        parse_last_exit(
            "\tstate = spawn scheduled\n\tlast terminating signal = Segmentation fault: 11"
        ),
        LastExit::Signal("Segmentation fault: 11".into())
    );
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
fn a_signal_outranks_an_exit_code_and_an_unreadable_one_outranks_nothing() {
    // A process that was signalled did not choose a code. If launchd ever
    // prints both, the signal is what happened to it.
    assert_eq!(
        parse_last_exit("\tlast exit code = 0\n\tlast terminating signal = Killed: 9"),
        LastExit::Signal("Killed: 9".into())
    );
    // But a signal line we cannot read must not fall through to an exit code
    // it contradicts, or a killed process reads as a clean one.
    for text in [
        "\tlast exit code = 0\n\tlast terminating signal = ",
        "\tlast exit code = 0\n\tlast terminating signal = Killed: 9\n\tlast terminating signal = Segmentation fault: 11",
    ] {
        assert_eq!(parse_last_exit(text), LastExit::Unknown, "{text}");
    }
}

#[test]
fn an_output_we_cannot_read_reports_nothing_rather_than_death() {
    // Nothing here accelerates a failure: `Unknown` is what keeps the caller
    // waiting for the deadline it was given.
    for text in [
        "gui/501/so.nessa.gateway.prod = {\n\tstate = running\n}",
        "\tlast exit code = not a number",
        "\tlast exit code = ",
        "\tlast exit code = : EX_CONFIG",
        "\tlast exit code = EX_CONFIG: 78",
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
    assert!(LastExit::Signal("Segmentation fault: 11".into()).is_failure());
    assert!(LastExit::Reason("JETSAM_REASON_MEMORY_IDLE_EXIT".into()).is_failure());
    assert!(!LastExit::Code(0).is_failure());
    assert!(!LastExit::NeverExited.is_failure());
    assert!(!LastExit::Unknown.is_failure());
}

/// `protocol/defaults/gateway-exit-codes.json`, read here the way the server
/// reads it, so a number changed in one place cannot pass this suite.
fn code(reason: &str) -> i32 {
    let table: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../protocol/defaults/gateway-exit-codes.json"
    ))
    .unwrap();
    table["codes"][reason].as_u64().unwrap() as i32
}

#[test]
fn the_registry_case_reads_as_a_registry_problem() {
    // The server chose this code deliberately, from its own typed error. The
    // log tail below is the v0.1.0 line that used to be the only evidence
    // there was; nothing reads it for meaning any more.
    let failure = diagnose(
        &LastExit::Code(code("credentialRegistryInvalid")),
        REGISTRY_LOG,
        PORT,
        READINESS,
    );
    assert_eq!(
        failure.sentence,
        "Nessa's background service is not starting: its credential registry is not one this version of Nessa can read."
    );
    // The readiness contract's own language is the thing this replaces.
    assert!(!failure.sentence.contains("runtime identity"));
    assert!(!failure.sentence.contains("forward recovery"));
    // And the evidence it was inferred from survives, for the app's log.
    assert!(failure
        .detail
        .contains(&format!("exit code {}", code("credentialRegistryInvalid"))));
    assert!(failure.detail.contains(READINESS));
    assert!(failure.detail.contains(REGISTRY_LOG));
}

#[test]
fn a_taken_port_names_the_port_the_service_was_registered_for() {
    let failure = diagnose(&LastExit::Code(code("portInUse")), "", PORT, READINESS);
    assert_eq!(
        failure.sentence,
        "Nessa's background service is not starting: port 7420 is already in use."
    );
}

#[test]
fn a_second_gateway_for_this_stage_is_told_that_and_not_blamed_on_the_registry() {
    // The registry lock is per stage and instance and held for the life of
    // the store, so it is what refuses a second gateway for a stage. Reported
    // as a registry fault it reads as corruption; it is the exclusion working.
    let failure = diagnose(&LastExit::Code(code("alreadyRunning")), "", PORT, READINESS);
    assert_eq!(
        failure.sentence,
        "Nessa's background service is not starting: another Nessa is already running for this stage."
    );
    assert!(!failure.sentence.contains("registry"));
}

#[test]
fn what_the_log_says_never_decides_what_the_panel_says() {
    // One append-only log covers every one of launchd's restarts, a healthy
    // launch writes about the same subsystems a failing one does, and every
    // one of these lines is a real line this server can emit. None of them
    // may answer for the failure: only the code the server exited with does.
    let misleading = [
        " INFO nessa_server::composition::provisioning: no local credential registry; creating one and an owner credential",
        REGISTRY_LOG,
        " INFO nessa_server: nessa server listening",
    ]
    .join("\n");
    let failure = diagnose(
        &LastExit::Code(code("portInUse")),
        &misleading,
        PORT,
        READINESS,
    );
    assert_eq!(
        failure.sentence,
        "Nessa's background service is not starting: port 7420 is already in use."
    );
    // The lines are still handed to the app's log, verbatim.
    assert!(failure.detail.contains(REGISTRY_LOG));

    // And the reverse: a log that says nothing about the registry does not
    // stop the registry being named when that is the code we were given.
    let quiet = diagnose(
        &LastExit::Code(code("credentialRegistryInvalid")),
        " INFO nessa_server: nessa server listening",
        PORT,
        READINESS,
    );
    assert_eq!(
        quiet.sentence,
        "Nessa's background service is not starting: its credential registry is not one this version of Nessa can read."
    );

    // A registry that is held rather than unreadable is not told it is the
    // wrong version, and neither is read out of the log.
    assert_eq!(
        diagnose(
            &LastExit::Code(code("credentialRegistry")),
            REGISTRY_LOG,
            PORT,
            READINESS
        )
        .sentence,
        "Nessa's background service is not starting: its credential registry could not be read."
    );
}

#[test]
fn a_program_that_never_ran_is_not_reported_as_something_it_said() {
    // launchd answers a plist whose program is missing with EX_CONFIG, not
    // with the 126 or 127 a shell would use. This is the case the issue asked
    // for by name, and it is the one that used to wait out the whole deadline.
    for exit in [LastExit::Code(78), LastExit::Code(126), LastExit::Code(127)] {
        assert_eq!(
            diagnose(&exit, "", PORT, READINESS).sentence,
            "Nessa's background service is not starting: its background program could not be launched."
        );
    }
    // Killed rather than exited: the signal is named in the log, not on screen.
    let killed = diagnose(
        &LastExit::Signal("Segmentation fault: 11".into()),
        "",
        PORT,
        READINESS,
    );
    assert_eq!(
        killed.sentence,
        "Nessa's background service is not starting: its background program stopped abruptly."
    );
    assert!(killed.detail.contains("Segmentation fault: 11"));
}

#[test]
fn the_formats_that_used_to_wait_out_the_deadline_now_fail_fast() {
    // Each of these is a real `launchctl print` fragment for a service that is
    // gone. Every one of them must be a failure, or readiness sits for thirty
    // seconds and then blames the runtime identity.
    for text in [
        "\tlast exit code = 78: EX_CONFIG",
        "\tlast terminating signal = Segmentation fault: 11",
        "\tlast terminating signal = Killed: 9",
        "\tlast exit code = 1",
    ] {
        let exit = parse_last_exit(text);
        assert!(exit.is_failure(), "{text}");
        assert_ne!(exit, LastExit::Unknown, "{text}");
    }
}

#[test]
fn a_reason_we_were_not_given_still_says_less_than_the_readiness_contract_did() {
    // The unclassified failure, a code no table entry claims, and an exit the
    // process did not choose at all: each is a service that is not starting,
    // and none of them invents a cause.
    for (exit, tail, expected) in [
        (
            LastExit::Code(1),
            "",
            "Nessa's background service is not starting, and it exited without reporting why.",
        ),
        (
            LastExit::Code(99),
            "thread 'main' panicked at src/x.rs",
            "Nessa's background service is not starting.",
        ),
        (
            LastExit::Reason("JETSAM_REASON_MEMORY_IDLE_EXIT".into()),
            "thread 'main' panicked at src/x.rs",
            "Nessa's background service is not starting.",
        ),
    ] {
        let failure = diagnose(&exit, tail, PORT, READINESS);
        assert_eq!(failure.sentence, expected);
        assert!(failure.detail.contains("none reported"));
        if tail.is_empty() {
            assert!(!failure.detail.contains("log tail"));
        } else {
            assert!(failure.detail.contains(tail));
        }
    }
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
