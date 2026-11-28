use crate::cli::entrypoint::{parse, Command, LocalProvisioning, ServerOptions};
use nessa_agent_install::domain::AgentName;
use std::path::PathBuf;
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
        Command::Server(ServerOptions::new(LocalProvisioning::Manual))
    );
    assert_eq!(
        parse(&args(&["server", "--provision-local"])).unwrap(),
        Command::Server(ServerOptions::new(LocalProvisioning::Automatic))
    );
    let passphrase_path = if cfg!(windows) {
        r"C:\secrets\nessa_vault_passphrase"
    } else {
        "/run/secrets/nessa_vault_passphrase"
    };
    let other_path = if cfg!(windows) {
        r"C:\secrets\other"
    } else {
        "/run/secrets/other"
    };
    assert_eq!(
        parse(&args(&[
            "server",
            "--provision-local",
            "--vault-passphrase-file",
            passphrase_path,
        ]))
        .unwrap(),
        Command::Server(ServerOptions {
            provisioning: LocalProvisioning::Automatic,
            vault_passphrase_file: Some(PathBuf::from(passphrase_path)),
            runtime: None,
        })
    );
    assert_eq!(
        parse(&args(&[
            "server",
            "--vault-passphrase-file",
            passphrase_path,
            "--provision-local",
        ]))
        .unwrap(),
        Command::Server(ServerOptions {
            provisioning: LocalProvisioning::Automatic,
            vault_passphrase_file: Some(PathBuf::from(passphrase_path)),
            runtime: None,
        })
    );
    assert_eq!(
        parse(&args(&[
            "server",
            "--vault-passphrase-file",
            passphrase_path,
        ]))
        .unwrap(),
        Command::Server(ServerOptions {
            provisioning: LocalProvisioning::Manual,
            vault_passphrase_file: Some(PathBuf::from(passphrase_path)),
            runtime: None,
        })
    );
    assert_eq!(
        parse(&args(&[
            "server",
            "--provision-local",
            "--runtime",
            "/opt/nessa/runtime",
            "--vault-passphrase-file",
            passphrase_path,
        ]))
        .unwrap(),
        Command::Server(ServerOptions {
            provisioning: LocalProvisioning::Automatic,
            vault_passphrase_file: Some(PathBuf::from(passphrase_path)),
            runtime: Some(PathBuf::from("/opt/nessa/runtime")),
        })
    );
    for refused in [
        vec!["server", "--runtime"],
        vec!["server", "--runtime", "relative/runtime"],
        vec!["server", "--runtime", "/a", "--runtime", "/b"],
    ] {
        assert!(parse(&args(&refused)).is_err(), "{refused:?}");
    }
    assert!(crate::cli::entrypoint::HELP.contains("--vault-passphrase-file"));
    assert!(crate::cli::entrypoint::HELP.contains("--runtime DIR"));
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
        vec!["server", "--vault-passphrase-file"],
        vec!["server", "--vault-passphrase-file", "relative/path"],
        vec!["server", "--vault-passphrase-file", ""],
        vec![
            "server",
            "--vault-passphrase-file",
            if cfg!(windows) {
                "C:\\secrets\\a\0b"
            } else {
                "/run/secrets/a\0b"
            },
        ],
        vec![
            "server",
            "--vault-passphrase-file",
            passphrase_path,
            "--vault-passphrase-file",
            other_path,
        ],
        vec![
            "server",
            "--desktop-runtime",
            passphrase_path,
            "--provision-local",
        ],
        vec![
            "server",
            "--desktop-runtime",
            passphrase_path,
            "--vault-passphrase-file",
            other_path,
        ],
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
fn relay_serve_names_the_address_it_listens_on() {
    assert_eq!(
        parse(&args(&["relay", "serve", "--listen", "0.0.0.0:7444"])).unwrap(),
        Command::RelayServe {
            listen: "0.0.0.0:7444".into(),
        }
    );
    for refused in [
        vec!["relay"],
        vec!["relay", "serve"],
        vec!["relay", "serve", "0.0.0.0:7444"],
        vec!["relay", "serve", "--listen"],
        vec!["relay", "serve", "--listen", "--help"],
        vec!["relay", "serve", "--listen", "a:1", "extra"],
    ] {
        assert!(parse(&args(&refused)).is_err(), "{refused:?}");
    }
    assert!(crate::cli::entrypoint::HELP.contains("nessa relay serve --listen HOST:PORT"));
}

#[test]
fn github_connection_has_no_user_facing_stage_or_endpoint_override() {
    assert!(matches!(
        parse(&args(&["github", "connect", "nessalabs/nessa-agent"])),
        Ok(Command::GithubConnect {
            accept_gap: false,
            ..
        })
    ));
    assert!(matches!(
        parse(&args(&["github", "events", "nessalabs/nessa-agent"])),
        Ok(Command::GithubEvents { .. })
    ));
    assert!(matches!(
        parse(&args(&["github", "disconnect", "nessalabs/nessa-agent"])),
        Ok(Command::GithubDisconnect { .. })
    ));
    for extra in ["--stage", "--url", "--accept-gap"] {
        assert!(parse(&args(&["github", "events", "nessalabs/nessa-agent", extra])).is_err());
    }
}

#[test]
fn github_passphrases_use_stdin_and_never_an_argument_or_stage_override() {
    for verb in ["connect", "events", "disconnect"] {
        let args: Vec<String> = ["github", verb, "owner/repo", "--passphrase-stdin"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(parse(&args).is_ok());
        for extra in ["--stage", "private-passphrase", "--passphrase-stdin"] {
            let mut rejected = args.clone();
            rejected.push(extra.into());
            assert!(parse(&rejected).is_err());
        }
    }
}

#[test]
fn call_and_mcp_take_a_method_or_only_a_credential_file() {
    let credential = std::env::temp_dir().join("cli.token");
    let credential = credential.to_str().expect("temp path is unicode");
    assert_eq!(
        parse(&args(&[
            "call",
            "conversation.list",
            "--params",
            "{}",
            "--credential-file",
            credential
        ]))
        .unwrap(),
        Command::Call {
            method: "conversation.list".into(),
            params: "{}".into(),
            credential_file: Some(credential.into()),
        }
    );
    assert!(matches!(
        parse(&args(&["mcp"])).unwrap(),
        Command::Mcp {
            credential_file: None
        }
    ));
    for words in [
        vec!["call"],
        vec!["call", "conversation.list"],
        vec!["call", "--params", "{}"],
        vec!["call", "conversation.list", "--params", "-1"],
        vec!["mcp", "--credential-file", "relative"],
        vec!["mcp", "--params", "{}"],
    ] {
        assert!(parse(&args(&words)).is_err(), "{words:?}");
    }
}
