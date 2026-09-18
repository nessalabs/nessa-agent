use crate::cli::entrypoint::{parse, Command};
fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| (*s).into()).collect()
}
#[test]
fn local_bootstrap_is_explicit_and_cloud_never_falls_back() {
    assert!(matches!(
        parse(&args(&["auth", "init", "--local"])),
        Ok(Command::Offline(_))
    ));
    assert!(matches!(
        parse(&args(&["auth", "token"])),
        Ok(Command::Token {
            credential_file: None,
            ttl_seconds: None
        })
    ));
    assert!(matches!(
        parse(&args(&["doctor", "--local"])),
        Ok(Command::Doctor {
            credential_file: None
        })
    ));
    assert_eq!(parse(&args(&["server"])).unwrap(), Command::Server);
    assert_eq!(parse(&[]).unwrap(), Command::Help);
    for words in [
        vec!["auth", "init"],
        vec!["auth", "init", "--cloud"],
        vec!["auth", "token", "--cloud"],
        vec!["doctor", "--local", "--cloud"],
        vec!["doctor", "--local", "--local"],
        vec!["auth", "token", "--credential-file", "relative"],
        vec!["server", "extra"],
        vec!["desktop-service", "/tmp"],
    ] {
        assert!(parse(&args(&words)).is_err(), "{words:?}");
    }
}

#[test]
fn token_ttl_is_explicit_bounded_and_exclusive_with_no_expiry() {
    for (text, seconds) in [("1s", 1), ("30m", 1800), ("12h", 43200), ("7d", 604800)] {
        assert!(
            matches!(parse(&args(&["auth","token","--ttl",text])),Ok(Command::Token {ttl_seconds:Some(value),..}) if value==seconds)
        );
    }
    for words in [
        vec!["auth", "token", "--ttl", "0d"],
        vec!["auth", "token", "--ttl", "-1d"],
        vec!["auth", "token", "--ttl", "1"],
        vec!["auth", "token", "--ttl", "18446744073709551615d"],
        vec!["auth", "token", "--ttl", "1d", "--no-expiry"],
        vec!["auth", "token", "--no-expiry", "--no-expiry"],
        vec!["doctor", "--ttl", "1d"],
    ] {
        assert!(parse(&args(&words)).is_err());
    }
}

#[test]
fn install_agent_names_one_agent() {
    assert_eq!(
        parse(&args(&["install-agent", "opencode"])),
        Ok(Command::InstallAgent {
            agent: "opencode".into()
        })
    );
}

#[test]
fn install_agent_refuses_a_name_that_could_be_a_path() {
    // The name goes on to be a directory under Nessa's data root. Refusing it
    // here means a mistyped command never reaches the filesystem at all.
    for hostile in ["..", "../escape", "/etc", "a/b", "Opencode", ""] {
        assert!(
            parse(&args(&["install-agent", hostile])).is_err(),
            "install-agent {hostile:?} should be refused"
        );
    }
}

#[test]
fn install_agent_wants_exactly_one_agent() {
    assert!(parse(&args(&["install-agent"])).is_err());
    assert!(parse(&args(&["install-agent", "opencode", "codex"])).is_err());
}

#[test]
fn the_help_text_mentions_installing_an_agent() {
    // The command exists to be discovered by someone reading `nessa --help`.
    assert!(crate::cli::entrypoint::HELP.contains("install-agent"));
}
