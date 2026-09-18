use crate::cli::entrypoint::{parse, Command, LocalProvisioning};
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
    assert_eq!(
        parse(&args(&["server"])).unwrap(),
        Command::Server(LocalProvisioning::Manual)
    );
    assert_eq!(
        parse(&args(&["server", "--provision-local"])).unwrap(),
        Command::Server(LocalProvisioning::Automatic)
    );
    assert_eq!(parse(&[]).unwrap(), Command::Help);
    for words in [
        vec!["auth", "init"],
        vec!["auth", "init", "--cloud"],
        vec!["auth", "token", "--cloud"],
        vec!["doctor", "--local", "--cloud"],
        vec!["doctor", "--local", "--local"],
        vec!["auth", "token", "--credential-file", "relative"],
        vec!["server", "extra"],
        vec!["server", "--provision-local", "--provision-local"],
        vec!["server", "--provision-local", "extra"],
        vec!["--provision-local"],
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
