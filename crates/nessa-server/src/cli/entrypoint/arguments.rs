use std::path::PathBuf;

use crate::agent_install::domain::AgentName;

pub const HELP: &str = "Nessa\n\n  nessa server [--provision-local]\n  nessa auth init --local [--owner-token-file PATH]\n  nessa auth token [--local] [--ttl 12h|7d|30m|60s | --no-expiry] [--credential-file PATH]\n  nessa doctor [--local] [--credential-file PATH]\n  nessa install-agent NAME\n  nessa limits [--json]\n  nessa env serve\n  nessa artifact publish PATH [--type MEDIA/TYPE]\n  nessa auth provision-surface --local --surface-id NAME [--grants ACTIONS]\n  nessa auth recover-owner --local --owner-token-file PATH\n\ninstall-agent downloads the release of an agent's own runtime that Nessa has\ntested, verifies it against a compiled-in digest, and reports what it installed\nas JSON on stdout. Installing one already present downloads nothing.\n\n`env serve` is the environment role a gateway starts over SSH (`ssh HOST nessa\nenv serve`): it speaks lease frames on stdin and stdout and runs the agent\nharnesses this host's own config.json configures. Not for people.\n\n`artifact publish` attaches a file in the workspace to the conversation whose\nagent runs it, from a host serving `env serve`: the gateway receives it by\ndigest and verifies it. It prints the outcome as JSON on stdout.\n\n`limits` and `limits --json` print the effective operational limits as JSON\non stdout. A missing config file prints the defaults. An unusable config\nfile prints nothing and fails.\n\nLocal is the current backend. Cloud auth is not implemented.\n`server --provision-local` creates the owner and panel credentials of the selected\nnamespace when they are absent, then serves; it never replaces existing ones.\nWithout it, `nessa server` only serves what the offline auth commands provisioned.\nNESSA_HOST, NESSA_PORT, NESSA_STAGE, NESSA_DATA_DIR and NESSA_INSTANCE select the local gateway.\nToken defaults to no expiry (capped by issuer expiry) and prints only the secret to stdout; pipe it to pbcopy.\n";

/// Whether a serving process may create the local credentials it needs.
///
/// Creating them mints an owner credential and writes it to disk, which is a
/// decision an operator makes, not a side effect of starting a server. A
/// developer's local loop and the packaged desktop app ask for it explicitly;
/// a plain `nessa server` never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalProvisioning {
    /// Serve only what the offline `auth` commands already provisioned.
    Manual,
    /// Create the owner and panel credentials when absent, then serve. Existing
    /// credentials are never rotated or replaced.
    Automatic,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Server(LocalProvisioning),
    Desktop(PathBuf),
    Offline(Vec<String>),
    Token {
        credential_file: Option<PathBuf>,
        ttl_seconds: Option<u64>,
    },
    Doctor {
        credential_file: Option<PathBuf>,
    },
    /// Put an agent's own runtime on this machine, at the tested version.
    InstallAgent {
        /// The agent as Nessa names it, such as `opencode`. Read into its value
        /// object here, at the edge, so that nothing further in has a name it
        /// still has to doubt — and so that a person who mistypes one gets the
        /// answer before anything is downloaded.
        agent: AgentName,
    },
    /// Print the effective operational limits as JSON on stdout.
    Limits,
    /// Serve a gateway's leases on stdin and stdout: the environment role,
    /// which a gateway starts as `ssh <host> nessa env serve` (#699).
    EnvServe,
    /// Publish a file to the conversation whose agent runs this command on
    /// a host serving `env serve` (#701), through its lease's publish point.
    ArtifactPublish {
        /// The file, as given.
        path: PathBuf,
        /// Its media type, when given; else read from its extension.
        media_type: Option<String>,
    },
    /// Stand in for a configured MCP server in a harness's session (ADR 344):
    /// relay this process's stdin and stdout to the gateway's one connection
    /// to that server. Not for people: the gateway gives it to the harness.
    McpRelay {
        /// The gateway's relay socket, absolute.
        socket: PathBuf,
        /// The configured server's name.
        server: String,
        /// The digest of the server's configuration when the stand-in was made.
        configuration: String,
    },
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "--cloud") {
        return Err("Cloud authentication is not implemented yet; use --local".into());
    }
    if args == ["env", "serve"] {
        return Ok(Command::EnvServe);
    }
    if let [artifact, publish, rest @ ..] = args {
        if artifact == "artifact" && publish == "publish" {
            return match rest {
                // A path never begins with `-`: a flag is never read as one.
                [path] if !path.starts_with('-') => Ok(Command::ArtifactPublish {
                    path: path.into(),
                    media_type: None,
                }),
                [path, flag, media_type] | [flag, media_type, path]
                    if flag == "--type" && !path.starts_with('-') =>
                {
                    Ok(Command::ArtifactPublish {
                        path: path.into(),
                        media_type: Some(media_type.clone()),
                    })
                }
                _ => {
                    Err("artifact publish takes one PATH and an optional --type MEDIA/TYPE".into())
                }
            };
        }
    }
    if args == ["server"] {
        return Ok(Command::Server(LocalProvisioning::Manual));
    }
    if args == ["server", "--provision-local"] {
        return Ok(Command::Server(LocalProvisioning::Automatic));
    }
    if let [relay, socket, server, configuration] = args {
        if relay == crate::mcp_servers::domain::RELAY_SUBCOMMAND {
            let socket = PathBuf::from(socket);
            if !socket.is_absolute() {
                return Err("mcp-relay takes an absolute socket path".into());
            }
            return Ok(Command::McpRelay {
                socket,
                server: server.clone(),
                configuration: configuration.clone(),
            });
        }
    }
    if let [install, rest @ ..] = args {
        if install == "install-agent" {
            let [agent] = rest else {
                return Err("install-agent takes one agent name, such as opencode".into());
            };
            let agent = AgentName::parse(agent).map_err(|error| error.to_string())?;
            return Ok(Command::InstallAgent { agent });
        }
    }
    if let [limits, rest @ ..] = args {
        if limits == "limits" {
            if rest.is_empty() || rest == ["--json"] {
                return Ok(Command::Limits);
            }
            return Err("limits takes only --json".into());
        }
    }
    if let [server, flag, directory] = args {
        if server == "server"
            && flag == "--desktop-runtime"
            && PathBuf::from(directory).is_absolute()
        {
            return Ok(Command::Desktop(directory.into()));
        }
    }
    let (token, rest) = match args {
        [auth, token, rest @ ..] if auth == "auth" && token == "token" => (true, rest),
        [doctor, rest @ ..] if doctor == "doctor" => (false, rest),
        [auth, operation, rest @ ..]
            if auth == "auth"
                && matches!(
                    operation.as_str(),
                    "init" | "recover-owner" | "provision-surface"
                ) =>
        {
            if rest.iter().filter(|v| *v == "--local").count() != 1 {
                return Err(
                    "Offline authentication requires --local; cloud auth is not implemented".into(),
                );
            }
            let mut command = vec![auth.clone(), operation.clone()];
            command.extend(rest.iter().filter(|v| *v != "--local").cloned());
            return Ok(Command::Offline(command));
        }
        _ => return Err(HELP.into()),
    };
    let mut credential_file = None;
    let mut local = false;
    let mut lifetime_set = false;
    let mut ttl_seconds = None;
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--local" if !local => local = true,
            "--no-expiry" if token && !lifetime_set => {
                lifetime_set = true;
            }
            "--ttl" if token && !lifetime_set => {
                lifetime_set = true;
                let value = words
                    .next()
                    .ok_or("--ttl requires a duration such as 12h or 7d")?;
                let split = value.len().checked_sub(1).ok_or("invalid TTL")?;
                let (number, unit) = value.split_at_checked(split).ok_or("invalid TTL")?;
                let factor = match unit {
                    "s" => 1,
                    "m" => 60,
                    "h" => 3600,
                    "d" => 86400,
                    _ => return Err("TTL unit must be s, m, h or d".into()),
                };
                if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("TTL must be a positive integer with a unit".into());
                }
                ttl_seconds = Some(
                    number
                        .parse::<u64>()
                        .ok()
                        .and_then(|v| v.checked_mul(factor))
                        .filter(|v| *v > 0)
                        .ok_or("TTL must be positive and representable")?,
                );
            }
            "--credential-file" if credential_file.is_none() => {
                let path = PathBuf::from(
                    words
                        .next()
                        .ok_or("--credential-file requires an absolute path")?,
                );
                if !path.is_absolute() {
                    return Err("--credential-file requires an absolute path".into());
                }
                credential_file = Some(path);
            }
            _ => return Err("unknown or duplicate option".into()),
        }
    }
    Ok(if token {
        Command::Token {
            credential_file,
            ttl_seconds,
        }
    } else {
        Command::Doctor { credential_file }
    })
}
