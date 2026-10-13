use std::path::PathBuf;

use crate::agent_install::domain::AgentName;
use nessa_protocol::relay::RelayAddress;

pub const HELP: &str = "Nessa\n\n  nessa server [--provision-local]\n  nessa auth init --local [--owner-token-file PATH]\n  nessa auth token [--local] [--ttl 12h|7d|30m|60s | --no-expiry] [--credential-file PATH]\n  nessa doctor [--local] [--credential-file PATH]\n  nessa install-agent NAME\n  nessa limits [--json]\n  nessa env serve\n  nessa env enroll ADDRESS CODE\n  nessa env dial ADDRESS [--relay HOST:PORT] [--drain-deadline 20s] [--enroll-code-file PATH]\n  nessa env grant [--peer ssh|PIN] [--agent NAME... | --all-agents] [--commands --max-timeout 10m] [--max-lease 8h]\n  nessa env ungrant [--peer ssh|PIN] (--agent NAME | --commands | --all)\n  nessa env grants\n  nessa env protocol\n  nessa artifact publish PATH [--type MEDIA/TYPE]\n  nessa relay serve --listen HOST:PORT\n  nessa auth provision-surface --local --surface-id NAME [--grants ACTIONS]\n  nessa auth recover-owner --local --owner-token-file PATH\n\ninstall-agent downloads the release of an agent's own runtime that Nessa has\ntested, verifies it against a compiled-in digest, and reports what it installed\nas JSON on stdout. Installing one already present downloads nothing.\n\n`env serve` is the environment role a gateway starts over SSH, from the copy of\nits own build it installs under ~/.nessa/env on first use: it speaks lease frames on stdin and stdout and runs the agent\nharnesses this host's own config.json configures. Not for people.\n`env enroll` pairs this machine with the gateway at ADDRESS (host:port of its\nnative listener) as an environment, with the code of an environment invitation\nthe owner created there; the owner then approves it. `env dial` serves that\ngateway's leases over connections this machine dials, waiting for the\napproval first and dialing again whenever a connection ends, until stopped.\nWith --relay, a dial that does not connect to ADDRESS within a moment, or is\nrefused, asks that relay for the gateway instead (`relay serve`, below);\nenrolling still needs ADDRESS itself.\nWith --drain-deadline, SIGINT or SIGTERM drains instead: every lease ends with\nits cleanup recorded on both sides, then it exits; past the deadline, or on a\nsecond signal, the connection is closed. Set it below a container's stop grace.\nWith --enroll-code-file, a machine paired with nothing first enrolls with the\ncode that file holds, and one already paired with the gateway at ADDRESS goes\nstraight to dialing, so a container can run the same command on every start;\none paired with another gateway refuses.\n`env grant` and `env ungrant` change what this machine's owner grants one\ngateway: `ssh` (whoever runs env serve over SSH) or a dialed gateway by its\npin, by default the gateway this machine is paired with, else `ssh`. Without\nan entry, `ssh` is granted everything this host runs and a dialed gateway\nnothing; a grant only narrows what config.json lets this host run. A change\napplies from the next lease; leases already running go on. `env grants`\nprints every entry as JSON on stdout. `env grant` takes agents, commands, a\nlease limit or any of them; --max-timeout replaces the peer's longest command\nrun, and --max-lease the longest an agent's lease may last before this machine\nends it (none: until ended; `ungrant --all` takes it back).\n`env protocol` prints the lease protocol this build speaks, which a gateway\nchecks a copy it installs on a host against before putting it in place.\n\n`artifact publish` attaches a file in the workspace to the conversation whose\nagent runs it, from a host serving `env serve`: the gateway receives it by\ndigest and verifies it. It prints the outcome as JSON on stdout.\n\n`relay serve` runs a relay at HOST:PORT that a gateway and its paired readers\nboth connect out to, so a reader that cannot reach the gateway directly is\njoined to it there. The relay copies their bytes and cannot read them: what\ncrosses is the same TLS a direct connection carries. It keeps nothing and runs\nuntil stopped.\n\n`limits` and `limits --json` print the effective operational limits as JSON\non stdout. A missing config file prints the defaults. An unusable config\nfile prints nothing and fails.\n\nLocal is the current backend. Cloud auth is not implemented.\n`server --provision-local` creates the owner and panel credentials of the selected\nnamespace when they are absent, then serves; it never replaces existing ones.\nWithout it, `nessa server` only serves what the offline auth commands provisioned.\nNESSA_HOST, NESSA_PORT, NESSA_STAGE, NESSA_DATA_DIR and NESSA_INSTANCE select the local gateway.\nToken defaults to no expiry (capped by issuer expiry) and prints only the secret to stdout; pipe it to pbcopy.\n";

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
    /// which a gateway starts over SSH from the copy of its build it installed
    /// there (#699, #703).
    EnvServe,
    /// Pair this machine with a gateway as an environment it cannot reach,
    /// with the code of an environment invitation (#706).
    EnvEnroll {
        /// The gateway's native listener, `host:port`.
        address: String,
        /// The invitation's code, as the owner gave it.
        code: String,
    },
    /// Serve a paired gateway's leases over connections this machine dials,
    /// until stopped (#706).
    EnvDial {
        /// The gateway's native listener, `host:port`.
        address: String,
        /// With it, a dial that does not connect directly asks this relay
        /// for the gateway (`docs/design/auth/relay.md`).
        relay: Option<RelayAddress>,
        /// With it, SIGINT or SIGTERM drains: every lease ends with its
        /// cleanup recorded on both sides, and past this long the connection
        /// is closed. Without it, they close the connection at once (#708).
        drain_deadline_ms: Option<u64>,
        /// With it, a machine paired with nothing first enrolls with the code
        /// this file holds, so a container's entrypoint can be the same on
        /// every start; one already paired with another gateway refuses.
        enroll_code_file: Option<PathBuf>,
    },
    /// Grant a gateway leases on this machine, as its owner (#706).
    EnvGrant {
        /// `ssh` or a gateway's pin; `None` for the default.
        peer: Option<String>,
        /// The agents granted, if any are.
        agents: Option<GrantedAgents>,
        /// Commands granted, at most this long each, if they are.
        max_timeout_ms: Option<u64>,
        /// How long each harness's lease may last, if this sets it.
        max_lease_ms: Option<u64>,
    },
    /// Take a grant back from a gateway, from its next lease (#706).
    EnvUngrant {
        /// `ssh` or a gateway's pin; `None` for the default.
        peer: Option<String>,
        what: Ungranted,
    },
    /// Print what this machine's owner grants each gateway, as JSON (#706).
    EnvGrants,
    /// Print the lease protocol this build speaks: how a gateway checks a
    /// copy of itself it installs on a host before putting it in place (#703).
    EnvProtocol,
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
    /// Run a relay gateways and their readers connect out to, joining a
    /// reader to its gateway when it cannot reach it directly (#266).
    RelayServe {
        /// The address to listen on, `host:port`.
        listen: String,
    },
}

/// The agents `env grant` grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantedAgents {
    /// Every agent this host runs.
    All,
    /// These, by name, as given.
    Named(Vec<String>),
}

/// What `env ungrant` takes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ungranted {
    /// One agent, by name.
    Agent(String),
    /// Commands.
    Commands,
    /// Everything.
    All,
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
    if args == ["env", "protocol"] {
        return Ok(Command::EnvProtocol);
    }
    if args == ["env", "grants"] {
        return Ok(Command::EnvGrants);
    }
    if let [env, verb, rest @ ..] = args {
        if env == "env" && verb == "grant" {
            return grant(rest);
        }
        if env == "env" && verb == "ungrant" {
            return ungrant(rest);
        }
    }
    if let [env, verb, rest @ ..] = args {
        if env == "env" && verb == "enroll" {
            return match rest {
                [address, code] if !address.starts_with('-') && !code.starts_with('-') => {
                    Ok(Command::EnvEnroll {
                        address: address.clone(),
                        code: code.clone(),
                    })
                }
                _ => Err("env enroll takes the gateway's ADDRESS and the invitation's CODE".into()),
            };
        }
        if env == "env" && verb == "dial" {
            return dial(rest);
        }
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
    if let [relay, serve, rest @ ..] = args {
        if relay == "relay" && serve == "serve" {
            return match rest {
                [flag, listen] if flag == "--listen" && !listen.starts_with('-') => {
                    Ok(Command::RelayServe {
                        listen: listen.clone(),
                    })
                }
                _ => Err("relay serve takes --listen HOST:PORT".into()),
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
                ttl_seconds = Some(duration_seconds(value).ok_or(
                    "--ttl requires a positive whole duration such as 60s, 30m, 12h or 7d",
                )?);
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

/// `env dial ADDRESS [--relay RELAY] [--drain-deadline DURATION]
/// [--enroll-code-file PATH]`: the flags in any order, each at most once.
fn dial(rest: &[String]) -> Result<Command, String> {
    const USAGE: &str = "env dial takes the gateway's ADDRESS [--relay HOST:PORT] \
                         [--drain-deadline DURATION] [--enroll-code-file PATH]";
    let [address, flags @ ..] = rest else {
        return Err(USAGE.into());
    };
    if address.starts_with('-') || flags.len() % 2 != 0 {
        return Err(USAGE.into());
    }
    let (mut relay, mut drain_deadline_ms, mut enroll_code_file) = (None, None, None);
    for pair in flags.chunks(2) {
        match (pair[0].as_str(), &pair[1]) {
            ("--relay", address) if relay.is_none() && !address.starts_with('-') => {
                relay = Some(RelayAddress::try_from(address.clone())?);
            }
            ("--drain-deadline", deadline) if drain_deadline_ms.is_none() => {
                drain_deadline_ms = Some(duration_ms(deadline)?);
            }
            // A path beginning with `-` is refused, so a flag is never read
            // as one; `./-code` names such a file.
            ("--enroll-code-file", path)
                if enroll_code_file.is_none() && !path.starts_with('-') =>
            {
                enroll_code_file = Some(PathBuf::from(path));
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Command::EnvDial {
        address: address.clone(),
        relay,
        drain_deadline_ms,
        enroll_code_file,
    })
}

/// `env grant`'s words after the verb.
fn grant(words: &[String]) -> Result<Command, String> {
    const USAGE: &str = "env grant takes --agent NAME (repeated) or --all-agents, \
                         --commands with --max-timeout DURATION, or --max-lease DURATION, \
                         and an optional --peer";
    let mut peer = None;
    let mut named = Vec::new();
    let mut all = false;
    let mut commands = false;
    let mut max_timeout_ms = None;
    let mut max_lease_ms = None;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--peer" if peer.is_none() => peer = Some(value(words.next(), USAGE)?),
            "--agent" => named.push(value(words.next(), USAGE)?),
            "--all-agents" if !all => all = true,
            "--commands" if !commands => commands = true,
            "--max-timeout" if max_timeout_ms.is_none() => {
                max_timeout_ms = Some(duration_ms(&value(words.next(), USAGE)?)?);
            }
            "--max-lease" if max_lease_ms.is_none() => {
                max_lease_ms = Some(duration_ms(&value(words.next(), USAGE)?)?);
            }
            _ => return Err(USAGE.into()),
        }
    }
    let agents = match (all, named.is_empty()) {
        (true, true) => Some(GrantedAgents::All),
        (false, false) => Some(GrantedAgents::Named(named)),
        (false, true) => None,
        (true, false) => return Err("env grant takes --agent or --all-agents, not both".into()),
    };
    if commands != max_timeout_ms.is_some()
        || (agents.is_none() && !commands && max_lease_ms.is_none())
    {
        return Err(USAGE.into());
    }
    Ok(Command::EnvGrant {
        peer,
        agents,
        max_timeout_ms,
        max_lease_ms,
    })
}

/// `env ungrant`'s words after the verb.
fn ungrant(words: &[String]) -> Result<Command, String> {
    const USAGE: &str =
        "env ungrant takes one of --agent NAME, --commands or --all, and an optional --peer";
    let mut peer = None;
    let mut what = None;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let taken = match word.as_str() {
            "--peer" if peer.is_none() => {
                peer = Some(value(words.next(), USAGE)?);
                continue;
            }
            "--agent" => Ungranted::Agent(value(words.next(), USAGE)?),
            "--commands" => Ungranted::Commands,
            "--all" => Ungranted::All,
            _ => return Err(USAGE.into()),
        };
        if what.replace(taken).is_some() {
            return Err(USAGE.into());
        }
    }
    let what = what.ok_or(USAGE)?;
    Ok(Command::EnvUngrant { peer, what })
}

/// A flag's value: present, and not another flag.
fn value(word: Option<&String>, usage: &str) -> Result<String, String> {
    word.filter(|word| !word.starts_with('-'))
        .cloned()
        .ok_or_else(|| usage.to_owned())
}

/// A positive whole number of seconds, minutes, hours or days (`90s`,
/// `10m`, `12h`, `7d`), in seconds; `None` for anything else, or too large.
fn duration_seconds(text: &str) -> Option<u64> {
    let (number, unit) = text.split_at_checked(text.len().checked_sub(1)?)?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return None,
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number
        .parse::<u64>()
        .ok()?
        .checked_mul(factor)
        .filter(|seconds| *seconds > 0)
}

/// [`duration_seconds`], in milliseconds.
fn duration_ms(text: &str) -> Result<u64, String> {
    duration_seconds(text)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .ok_or_else(|| format!("{text}: give a duration such as 90s, 10m or 1h"))
}
