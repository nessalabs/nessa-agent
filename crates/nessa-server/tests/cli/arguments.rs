use crate::agent_install::domain::AgentName;
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

#[test]
fn install_agent_names_one_agent() {
    assert_eq!(
        parse(&args(&["install-agent", "opencode"])),
        Ok(Command::InstallAgent {
            agent: AgentName::parse("opencode").expect("a plain agent name")
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
    // Named rather than answered with the whole help text: somebody who typed
    // the command already knows it exists and needs the one thing they left out.
    for words in [
        vec!["install-agent"],
        vec!["install-agent", "opencode", "codex"],
    ] {
        let refusal = parse(&args(&words)).expect_err("{words:?} names no single agent");
        assert!(
            refusal.contains("one agent name"),
            "unhelpful message for {words:?}: {refusal}"
        );
    }
}

#[test]
fn limits_prints_json_and_takes_nothing_else() {
    assert_eq!(parse(&args(&["limits"])), Ok(Command::Limits));
    assert_eq!(parse(&args(&["limits", "--json"])), Ok(Command::Limits));
    let extra = parse(&args(&["limits", "extra"])).expect_err("extra");
    assert!(extra.contains("only --json"), "{extra}");
    assert!(crate::cli::entrypoint::HELP.contains("nessa limits"));
}

#[test]
fn the_help_text_mentions_installing_an_agent() {
    // The command exists to be discovered by someone reading `nessa --help`.
    assert!(crate::cli::entrypoint::HELP.contains("install-agent"));
}

// A Unix socket's path: absolute as Unix spells it, which Windows does not.
#[cfg(unix)]
#[test]
fn a_stand_in_names_an_absolute_socket_a_server_and_its_configuration() {
    assert_eq!(
        parse(&args(&[
            "mcp-relay",
            "/ns/mcp/relay.sock",
            "mcptest",
            "sha256:ab"
        ])),
        Ok(Command::McpRelay {
            socket: "/ns/mcp/relay.sock".into(),
            server: "mcptest".into(),
            configuration: "sha256:ab".into(),
        })
    );
    assert!(parse(&args(&["mcp-relay", "relay.sock", "mcptest", "sha256:ab"])).is_err());
    assert!(parse(&args(&["mcp-relay", "/s", "mcptest"])).is_err());
    assert!(parse(&args(&["mcp-relay", "/s", "mcptest", "d", "extra"])).is_err());
}

#[test]
fn artifact_publish_names_one_path_and_optionally_its_type() {
    assert_eq!(
        parse(&args(&["artifact", "publish", "/work/report.pdf"])),
        Ok(Command::ArtifactPublish {
            path: "/work/report.pdf".into(),
            media_type: None,
        })
    );
    for words in [
        ["artifact", "publish", "/work/chart", "--type", "image/png"],
        ["artifact", "publish", "--type", "image/png", "/work/chart"],
    ] {
        assert_eq!(
            parse(&args(&words)),
            Ok(Command::ArtifactPublish {
                path: "/work/chart".into(),
                media_type: Some("image/png".into()),
            }),
            "{words:?}"
        );
    }
    for words in [
        vec!["artifact", "publish"],
        vec!["artifact"],
        vec!["artifact", "publish", "/a", "/b"],
        vec!["artifact", "publish", "/a", "--type"],
        vec!["artifact", "publish", "--type", "image/png"],
        vec!["artifact", "publish", "/a", "--kind", "image/png"],
        vec![
            "artifact",
            "publish",
            "/a",
            "--type",
            "image/png",
            "--type",
            "text/plain",
        ],
        vec!["artifact", "publish", "/a", "--type", "image/png", "extra"],
    ] {
        assert!(parse(&args(&words)).is_err(), "{words:?}");
    }
}

/// A flag where the path goes is a flag, not a file named like one.
#[test]
fn artifact_publish_takes_no_flag_as_its_path() {
    for words in [
        vec!["artifact", "publish", "--type"],
        vec!["artifact", "publish", "--json"],
        vec!["artifact", "publish", "--type", "image/png", "--type"],
    ] {
        assert!(parse(&args(&words)).is_err(), "{words:?}");
    }
}
