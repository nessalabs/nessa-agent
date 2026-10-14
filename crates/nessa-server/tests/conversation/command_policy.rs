//! The gateway's command configuration, read as facts: what it describes and
//! a program's file name. Which commands run where is the command policy's,
//! tested in `command_policy_cedar.rs`.
use super::{program_name, CommandPolicy};

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn a_program_is_named_by_its_file_name() {
    for (program, name) in [
        ("rm", "rm"),
        ("/usr/bin/rm", "rm"),
        ("./cargo", "cargo"),
        ("rm/", ""),
        ("", ""),
    ] {
        assert_eq!(program_name(program), name, "{program}");
    }
}

#[test]
fn a_policy_described_differs_whenever_what_it_grants_does() {
    let base = CommandPolicy::new(names(&["devbox"]), None, Vec::new());
    for other in [
        CommandPolicy::new(names(&["build"]), None, Vec::new()),
        CommandPolicy::new(names(&["devbox"]), Some(Vec::new()), Vec::new()),
        CommandPolicy::new(names(&["devbox"]), None, names(&["rm"])),
    ] {
        assert_ne!(base.describe(), other.describe());
    }
    assert_eq!(
        base.describe(),
        CommandPolicy::new(names(&["devbox", "devbox"]), None, Vec::new()).describe()
    );
}
