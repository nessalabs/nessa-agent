use super::*;

fn linux() -> ReleasePlatform {
    ReleasePlatform::new("linux", "x86_64").expect("a well formed platform")
}

/// A machine, written the way the tests below read best: what it is, which C
/// library it has, and whether its processor does AVX2.
fn machine(libc: Option<Libc>, avx2: bool) -> HostPlatform {
    HostPlatform::new(linux(), libc, avx2)
}

#[test]
fn a_build_with_no_requirements_runs_anywhere_on_its_platform() {
    let anything = ReleaseRequirements::default();
    for host in [
        machine(None, false),
        machine(None, true),
        machine(Some(Libc::Gnu), false),
        machine(Some(Libc::Musl), true),
    ] {
        assert!(
            host.satisfies(&anything),
            "{host} can run a build that asks for nothing"
        );
    }
}

#[test]
fn a_build_runs_only_against_the_c_library_it_was_linked_for() {
    let glibc = ReleaseRequirements::new(Some(Libc::Gnu), false);
    let musl = ReleaseRequirements::new(Some(Libc::Musl), false);

    assert!(machine(Some(Libc::Gnu), false).satisfies(&glibc));
    assert!(machine(Some(Libc::Musl), false).satisfies(&musl));
    // The pair that matters: each of these binaries dies in the loader on the
    // other's machine, before it prints anything a person could act on.
    assert!(!machine(Some(Libc::Musl), false).satisfies(&glibc));
    assert!(!machine(Some(Libc::Gnu), false).satisfies(&musl));
}

#[test]
fn a_machine_that_cannot_say_which_c_library_it_has_is_not_guessed_at() {
    // Not "it is probably glibc". A target linked against neither is one
    // nothing here established anything about, and installing a build that
    // names a library on it would be choosing on no evidence.
    for wanted in [Libc::Gnu, Libc::Musl] {
        assert!(
            !machine(None, false).satisfies(&ReleaseRequirements::new(Some(wanted), false)),
            "a machine with no known c library does not satisfy {wanted}"
        );
    }
}

#[test]
fn a_build_that_needs_avx2_runs_only_on_a_processor_that_has_it() {
    let avx2 = ReleaseRequirements::new(None, true);

    assert!(machine(None, true).satisfies(&avx2));
    // The whole reason this distinction exists: the other way round is not a
    // slow runtime, it is an illegal instruction on a machine that was just
    // told its runtime was ready.
    assert!(!machine(None, false).satisfies(&avx2));
}

#[test]
fn a_processor_with_avx2_still_runs_a_build_that_does_not_need_it() {
    // Which is what makes the choice between the two a preference rather than
    // a second filter: on such a machine both builds are eligible.
    assert!(machine(None, true).satisfies(&ReleaseRequirements::default()));
    assert!(machine(Some(Libc::Musl), true)
        .satisfies(&ReleaseRequirements::new(Some(Libc::Musl), false)));
}

#[test]
fn every_requirement_has_to_hold_at_once() {
    let musl_avx2 = ReleaseRequirements::new(Some(Libc::Musl), true);

    assert!(machine(Some(Libc::Musl), true).satisfies(&musl_avx2));
    // One right and one wrong is still a binary that does not run, in each
    // direction. A machine only has to fail one of them.
    assert!(!machine(Some(Libc::Musl), false).satisfies(&musl_avx2));
    assert!(!machine(Some(Libc::Gnu), true).satisfies(&musl_avx2));
}

#[test]
fn a_host_keeps_the_platform_it_was_built_with() {
    assert_eq!(machine(None, false).platform(), &linux());
}

#[test]
fn the_two_spellings_of_a_c_library_are_the_only_ones() {
    assert_eq!(Libc::parse("gnu"), Ok(Libc::Gnu));
    assert_eq!(Libc::parse("musl"), Ok(Libc::Musl));
    for wrong in ["", "GNU", "glibc", "uclibc", " musl"] {
        assert_eq!(
            Libc::parse(wrong),
            Err(PinRejected::Libc(wrong.to_owned())),
            "{wrong:?} is not a c library nessa can check"
        );
    }
}

#[test]
fn what_a_build_needs_reads_as_a_sentence() {
    // These go into the message a person sees when nothing is pinned for their
    // machine, so each combination is checked rather than assumed.
    assert_eq!(
        ReleaseRequirements::default().to_string(),
        "no special requirements"
    );
    assert_eq!(ReleaseRequirements::new(None, true).to_string(), "avx2");
    assert_eq!(
        ReleaseRequirements::new(Some(Libc::Gnu), false).to_string(),
        "gnu"
    );
    assert_eq!(
        ReleaseRequirements::new(Some(Libc::Musl), true).to_string(),
        "musl and avx2"
    );
}

#[test]
fn a_machine_reads_as_a_sentence_too() {
    assert_eq!(
        machine(Some(Libc::Gnu), true).to_string(),
        "linux-x86_64 with gnu and avx2"
    );
    assert_eq!(
        machine(Some(Libc::Musl), false).to_string(),
        "linux-x86_64 with musl and no avx2"
    );
    // The absences are said out loud, because the one place this is shown to a
    // person is the message saying nothing is pinned for their machine — and
    // "no tested release for linux-x86_64" on a platform pinned four times
    // over sends them looking for the wrong thing entirely.
    assert_eq!(
        machine(None, false).to_string(),
        "linux-x86_64 with no c library nessa could name and no avx2"
    );
    assert_eq!(
        machine(None, true).to_string(),
        "linux-x86_64 with no c library nessa could name and avx2"
    );
}

#[test]
fn two_builds_one_machine_can_both_run_never_ask_the_same_amount_of_it() {
    // What makes choosing between them independent of the order they are
    // listed in. Enumerated rather than argued: every machine this can
    // describe, against every build it can describe, asserting that no two
    // distinct builds a single machine runs are ranked equal.
    //
    // A tie would mean the choice fell back to the order of the pin file,
    // which is the outcome `PinFileError::PlatformPinnedTwice` exists to
    // prevent and which a ranking on AVX2 alone would allow: a build naming no
    // C library and one naming this machine's library are both runnable, and
    // differ.
    let libcs = [None, Some(Libc::Gnu), Some(Libc::Musl)];
    let every_build: Vec<ReleaseRequirements> = libcs
        .iter()
        .flat_map(|libc| [false, true].map(|avx2| ReleaseRequirements::new(*libc, avx2)))
        .collect();

    for libc in libcs {
        for avx2 in [false, true] {
            let host = machine(libc, avx2);
            let runnable: Vec<_> = every_build
                .iter()
                .filter(|build| host.satisfies(build))
                .collect();
            for (index, build) in runnable.iter().enumerate() {
                for other in &runnable[..index] {
                    assert_ne!(
                        build.demand(),
                        other.demand(),
                        "{host} runs both {build} and {other}, and ranks them equal"
                    );
                }
            }
        }
    }
}

#[test]
fn asking_more_of_a_machine_ranks_higher() {
    // The direction of the order, stated once so the ranking above is not just
    // "some total order". A build that needs AVX2 outranks one that does not,
    // and a build that names a C library outranks one that names none.
    assert!(
        ReleaseRequirements::new(None, true).demand()
            > ReleaseRequirements::new(None, false).demand()
    );
    assert!(
        ReleaseRequirements::new(Some(Libc::Gnu), false).demand()
            > ReleaseRequirements::new(None, false).demand()
    );
}
