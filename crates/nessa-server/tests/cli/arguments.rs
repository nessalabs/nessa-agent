use crate::agent_install::domain::AgentName;
use crate::cli::entrypoint::{parse, Command, GrantedAgents, LocalProvisioning, Ungranted};
use nessa_protocol::relay::RelayAddress;
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

#[test]
fn env_enroll_names_a_gateway_and_a_code_and_env_dial_a_gateway() {
    assert_eq!(
        parse(&args(&["env", "enroll", "gw.example:7443", "ABCD-EFGH"])).unwrap(),
        Command::EnvEnroll {
            address: "gw.example:7443".into(),
            code: "ABCD-EFGH".into(),
        }
    );
    assert_eq!(
        parse(&args(&["env", "dial", "10.0.0.2:7443"])).unwrap(),
        Command::EnvDial {
            address: "10.0.0.2:7443".into(),
            relay: None,
            drain_deadline_ms: None,
            enroll_code_file: None,
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "dial",
            "10.0.0.2:7443",
            "--drain-deadline",
            "20s"
        ]))
        .unwrap(),
        Command::EnvDial {
            address: "10.0.0.2:7443".into(),
            relay: None,
            drain_deadline_ms: Some(20_000),
            enroll_code_file: None,
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "dial",
            "gw.example:7443",
            "--enroll-code-file",
            "/run/secrets/nessa_code",
            "--drain-deadline",
            "20s",
        ]))
        .unwrap(),
        Command::EnvDial {
            address: "gw.example:7443".into(),
            relay: None,
            drain_deadline_ms: Some(20_000),
            enroll_code_file: Some("/run/secrets/nessa_code".into()),
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "dial",
            "10.0.0.2:7443",
            "--drain-deadline",
            "20s",
            "--relay",
            "relay.example:7444",
        ]))
        .unwrap(),
        Command::EnvDial {
            address: "10.0.0.2:7443".into(),
            relay: Some(RelayAddress::try_from("relay.example:7444".to_owned()).unwrap()),
            drain_deadline_ms: Some(20_000),
            enroll_code_file: None,
        }
    );
    for refused in [
        vec!["env", "enroll"],
        vec!["env", "enroll", "gw:1"],
        vec!["env", "enroll", "gw:1", "CODE", "extra"],
        vec!["env", "enroll", "--address", "gw:1"],
        vec!["env", "dial"],
        vec!["env", "dial", "gw:1", "extra"],
        vec!["env", "dial", "--help"],
        vec!["env", "dial", "gw:1", "--drain-deadline"],
        vec!["env", "dial", "gw:1", "--drain-deadline", "soon"],
        vec!["env", "dial", "--drain-deadline", "20s", "gw:1"],
        vec!["env", "dial", "gw:1", "--enroll-code-file"],
        vec!["env", "dial", "gw:1", "--relay"],
        vec!["env", "dial", "gw:1", "--relay", "no-port"],
        vec!["env", "dial", "gw:1", "--relay", "--drain-deadline"],
        vec!["env", "dial", "gw:1", "--relay", "r:1", "--relay", "r:2"],
        vec![
            "env",
            "dial",
            "gw:1",
            "--enroll-code-file",
            "--drain-deadline",
        ],
        vec![
            "env",
            "dial",
            "gw:1",
            "--enroll-code-file",
            "a",
            "--enroll-code-file",
            "b",
        ],
        vec![
            "env",
            "dial",
            "gw:1",
            "--drain-deadline",
            "1s",
            "--drain-deadline",
            "2s",
        ],
    ] {
        assert!(parse(&args(&refused)).is_err(), "{refused:?}");
    }
    assert!(crate::cli::entrypoint::HELP.contains("nessa env enroll ADDRESS CODE"));
    assert!(crate::cli::entrypoint::HELP.contains("nessa env dial ADDRESS [--relay HOST:PORT]"));
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
fn env_grant_names_agents_or_every_agent_and_commands_with_their_longest_run() {
    assert_eq!(
        parse(&args(&[
            "env", "grant", "--agent", "claude", "--agent", "codex"
        ]))
        .unwrap(),
        Command::EnvGrant {
            peer: None,
            agents: Some(GrantedAgents::Named(vec!["claude".into(), "codex".into()])),
            max_timeout_ms: None,
            max_lease_ms: None,
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "grant",
            "--peer",
            "ssh",
            "--all-agents",
            "--commands",
            "--max-timeout",
            "10m",
        ]))
        .unwrap(),
        Command::EnvGrant {
            peer: Some("ssh".into()),
            agents: Some(GrantedAgents::All),
            max_timeout_ms: Some(600_000),
            max_lease_ms: None,
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "grant",
            "--commands",
            "--max-timeout",
            "90s"
        ]))
        .unwrap(),
        Command::EnvGrant {
            peer: None,
            agents: None,
            max_timeout_ms: Some(90_000),
            max_lease_ms: None,
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "grant",
            "--commands",
            "--max-timeout",
            "1h"
        ]))
        .unwrap(),
        Command::EnvGrant {
            peer: None,
            agents: None,
            max_timeout_ms: Some(3_600_000),
            max_lease_ms: None,
        }
    );
    assert_eq!(
        parse(&args(&["env", "grant", "--max-lease", "8h"])).unwrap(),
        Command::EnvGrant {
            peer: None,
            agents: None,
            max_timeout_ms: None,
            max_lease_ms: Some(28_800_000),
        }
    );
    assert_eq!(
        parse(&args(&[
            "env",
            "grant",
            "--agent",
            "claude",
            "--max-lease",
            "30m"
        ]))
        .unwrap(),
        Command::EnvGrant {
            peer: None,
            agents: Some(GrantedAgents::Named(vec!["claude".into()])),
            max_timeout_ms: None,
            max_lease_ms: Some(1_800_000),
        }
    );
    for refused in [
        vec!["env", "grant"],
        vec!["env", "grant", "--max-lease"],
        vec!["env", "grant", "--max-lease", "0s"],
        vec!["env", "grant", "--max-lease", "8"],
        vec!["env", "grant", "--max-lease", "1h", "--max-lease", "2h"],
        vec!["env", "grant", "--agent"],
        vec!["env", "grant", "--agent", "--all-agents"],
        vec!["env", "grant", "--agent", "claude", "--all-agents"],
        vec!["env", "grant", "--commands"],
        vec!["env", "grant", "--max-timeout", "10m"],
        vec!["env", "grant", "--commands", "--max-timeout", "10"],
        vec!["env", "grant", "--commands", "--max-timeout", "0s"],
        vec!["env", "grant", "--commands", "--max-timeout", "-5m"],
        vec!["env", "grant", "--commands", "--max-timeout", "1.5h"],
        vec!["env", "grant", "--commands", "--max-timeout", "m"],
        vec![
            "env",
            "grant",
            "--peer",
            "ssh",
            "--peer",
            "ssh",
            "--all-agents",
        ],
        vec!["env", "grant", "--all-agents", "extra"],
    ] {
        assert!(parse(&args(&refused)).is_err(), "{refused:?}");
    }
    assert!(crate::cli::entrypoint::HELP.contains(
        "nessa env grant [--peer ssh|PIN] [--agent NAME... | --all-agents] [--commands --max-timeout 10m] [--max-lease 8h]"
    ));
}

#[test]
fn env_ungrant_takes_one_thing_back_and_env_grants_lists_them() {
    assert_eq!(
        parse(&args(&["env", "ungrant", "--agent", "claude"])).unwrap(),
        Command::EnvUngrant {
            peer: None,
            what: Ungranted::Agent("claude".into()),
        }
    );
    assert_eq!(
        parse(&args(&["env", "ungrant", "--commands", "--peer", "ssh"])).unwrap(),
        Command::EnvUngrant {
            peer: Some("ssh".into()),
            what: Ungranted::Commands,
        }
    );
    assert_eq!(
        parse(&args(&["env", "ungrant", "--all"])).unwrap(),
        Command::EnvUngrant {
            peer: None,
            what: Ungranted::All,
        }
    );
    assert_eq!(
        parse(&args(&["env", "grants"])).unwrap(),
        Command::EnvGrants
    );
    for refused in [
        vec!["env", "ungrant"],
        vec!["env", "ungrant", "--peer", "ssh"],
        vec!["env", "ungrant", "--agent"],
        vec!["env", "ungrant", "--commands", "--all"],
        vec!["env", "ungrant", "--agent", "claude", "--agent", "codex"],
        vec!["env", "grants", "--json"],
    ] {
        assert!(parse(&args(&refused)).is_err(), "{refused:?}");
    }
    let help = crate::cli::entrypoint::HELP;
    assert!(help.contains("nessa env ungrant [--peer ssh|PIN] (--agent NAME | --commands | --all)"));
    assert!(help.contains("nessa env grants"));
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
