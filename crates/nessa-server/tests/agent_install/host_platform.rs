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
    assert_eq!(machine(None, false).to_string(), "linux-x86_64");
    assert_eq!(
        machine(Some(Libc::Musl), false).to_string(),
        "linux-x86_64 with musl"
    );
    assert_eq!(
        machine(Some(Libc::Gnu), true).to_string(),
        "linux-x86_64 with gnu and avx2"
    );
    assert_eq!(machine(None, true).to_string(), "linux-x86_64 and avx2");
}
