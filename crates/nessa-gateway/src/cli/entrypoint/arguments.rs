use std::path::PathBuf;

use nessa_agent_install::domain::AgentName;

pub const HELP: &str = "Nessa\n\n  nessa server [--provision-local] [--vault-passphrase-file PATH]\n  nessa auth init --local [--owner-token-file PATH]\n  nessa auth token [--local] [--ttl 12h|7d|30m|60s | --no-expiry] [--credential-file PATH]\n  nessa doctor [--local] [--credential-file PATH]\n  nessa call METHOD --params JSON [--credential-file PATH]\n  nessa mcp [--credential-file PATH]\n  nessa install-agent NAME\n  nessa github connect OWNER/REPO --passphrase-stdin [--accept-gap]\n  nessa github events OWNER/REPO --passphrase-stdin\n  nessa github disconnect OWNER/REPO --passphrase-stdin\n  nessa limits [--json]\n  nessa relay serve --listen HOST:PORT\n  nessa auth provision-surface --local --surface-id NAME [--grants ACTIONS]\n  nessa auth recover-owner --local --owner-token-file PATH\n\ninstall-agent downloads the release of an agent's own runtime that Nessa has\ntested, verifies it against a compiled-in digest, and reports what it installed\nas JSON on stdout. Installing one already present downloads nothing.\n\n`relay serve` runs a relay at HOST:PORT that a gateway and its paired readers\nboth connect out to, so a reader that cannot reach the gateway directly is\njoined to it there. The relay copies their bytes and cannot read them: what\ncrosses is the same TLS a direct connection carries. It keeps nothing and runs\nuntil stopped.\n\n`limits` and `limits --json` print the effective operational limits as JSON\non stdout. A missing config file prints the defaults. An unusable config\nfile prints nothing and fails.\n\nLocal is the current backend. Cloud auth is not implemented.\n`server --provision-local` creates the owner, panel, and nessa-mcp credentials of the selected\nnamespace when they are absent, then serves; it never replaces existing ones.\nWithout it, `nessa server` only serves what the offline auth commands provisioned.\n`server --vault-passphrase-file PATH` unlocks (or initializes on first boot) the\nnamespace's encrypted vault from an absolute, non-symlink file before listening,\nfor headless Linux servers and containers.\n`call` sends one manifest method on the signed-in gateway session and prints the\nhandler's JSON on stdout. `mcp` serves those same methods as MCP tools, using the\nnessa-mcp credential and only the grants that credential holds. Cedar on the gateway\ndecides both. A method the surface catalog excludes is not a call and not a tool.\nNESSA_HOST, NESSA_PORT, NESSA_STAGE, NESSA_DATA_DIR and NESSA_INSTANCE select the local gateway.\nToken defaults to no expiry (capped by issuer expiry) and prints only the secret to stdout; pipe it to pbcopy.\n";

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

/// Options parsed for `nessa server`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptions {
    pub provisioning: LocalProvisioning,
    /// Optional absolute path to a passphrase file used to initialize or unlock
    /// the encrypted vault on headless startup before binding listeners.
    pub vault_passphrase_file: Option<PathBuf>,
}

impl ServerOptions {
    pub fn new(provisioning: LocalProvisioning) -> Self {
        Self {
            provisioning,
            vault_passphrase_file: None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    /// Retain one app-owned encrypted-vault session over anonymous pipes.
    VaultSession,
    Help,
    /// Connect using the current gh account in this binary's namespace.
    GithubConnect {
        repository: String,
        accept_gap: bool,
        passphrase_stdin: bool,
    },
    /// Read the private local event inbox without contacting GitHub.
    GithubEvents {
        repository: String,
        passphrase_stdin: bool,
    },
    /// Remove this owner's local binding and retained events.
    GithubDisconnect {
        repository: String,
        passphrase_stdin: bool,
    },
    Server(ServerOptions),
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
    /// Send one manifest method on the signed-in gateway session.
    Call {
        method: String,
        /// JSON text of the method's params.
        params: String,
        credential_file: Option<PathBuf>,
    },
    /// Serve the manifest methods as an MCP server, on this process's stdio.
    Mcp {
        credential_file: Option<PathBuf>,
    },
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    if args == ["vault", "session"] {
        return Ok(Command::VaultSession);
    }
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "--cloud") {
        return Err("Cloud authentication is not implemented yet; use --local".into());
    }
    if let [github, verb, repository, rest @ ..] = args {
        if github == "github" && matches!(verb.as_str(), "connect" | "events" | "disconnect") {
            let connect = verb == "connect";
            if repository.starts_with('-')
                || rest.iter().any(|flag| {
                    flag != "--passphrase-stdin" && !(connect && flag == "--accept-gap")
                })
                || rest.iter().collect::<std::collections::HashSet<_>>().len() != rest.len()
            {
                return Err(
                    "github connect OWNER/REPO [--accept-gap] or github events OWNER/REPO".into(),
                );
            }
            return Ok(match verb.as_str() {
                "connect" => Command::GithubConnect {
                    repository: repository.clone(),
                    accept_gap: rest.iter().any(|flag| flag == "--accept-gap"),
                    passphrase_stdin: rest.iter().any(|flag| flag == "--passphrase-stdin"),
                },
                "disconnect" => Command::GithubDisconnect {
                    passphrase_stdin: rest.iter().any(|flag| flag == "--passphrase-stdin"),
                    repository: repository.clone(),
                },
                _ => Command::GithubEvents {
                    passphrase_stdin: rest.iter().any(|flag| flag == "--passphrase-stdin"),
                    repository: repository.clone(),
                },
            });
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
    if let [first, rest @ ..] = args {
        if first == "server" {
            if let [flag, directory] = rest {
                if flag == "--desktop-runtime" && PathBuf::from(directory).is_absolute() {
                    return Ok(Command::Desktop(directory.into()));
                }
            }
            return parse_server(rest).map(Command::Server);
        }
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
    if let [command, rest @ ..] = args {
        if command == "call" {
            return parse_call(rest);
        }
        if command == "mcp" {
            return parse_mcp(rest);
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

fn parse_call(rest: &[String]) -> Result<Command, String> {
    const USAGE: &str = "call takes METHOD --params JSON [--credential-file PATH]";
    let [method, flags @ ..] = rest else {
        return Err(USAGE.into());
    };
    if method.is_empty() || method.starts_with('-') || flags.len() % 2 != 0 {
        return Err(USAGE.into());
    }
    let mut params = None;
    let mut credential_file = None;
    for pair in flags.chunks(2) {
        match (pair[0].as_str(), pair[1].as_str()) {
            ("--params", value) if params.is_none() && !value.starts_with('-') => {
                params = Some(value.to_owned());
            }
            ("--credential-file", path) if credential_file.is_none() && !path.starts_with('-') => {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err("--credential-file requires an absolute path".into());
                }
                credential_file = Some(path);
            }
            _ => return Err(USAGE.into()),
        }
    }
    let Some(params) = params else {
        return Err(USAGE.into());
    };
    Ok(Command::Call {
        method: method.clone(),
        params,
        credential_file,
    })
}

fn parse_mcp(rest: &[String]) -> Result<Command, String> {
    const USAGE: &str = "mcp takes only an optional --credential-file PATH";
    if rest.len() > 2 || rest.len() == 1 {
        return Err(USAGE.into());
    }
    let credential_file = if rest.is_empty() {
        None
    } else if rest.len() == 2 && rest[0] == "--credential-file" && !rest[1].starts_with('-') {
        let path = PathBuf::from(&rest[1]);
        if !path.is_absolute() {
            return Err("--credential-file requires an absolute path".into());
        }
        Some(path)
    } else {
        return Err(USAGE.into());
    };
    Ok(Command::Mcp { credential_file })
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

fn parse_server(args: &[String]) -> Result<ServerOptions, String> {
    let mut provisioning = None;
    let mut vault_passphrase_file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--provision-local" => {
                if provisioning.is_some() {
                    return Err("server takes --provision-local at most once".into());
                }
                provisioning = Some(LocalProvisioning::Automatic);
                index += 1;
            }
            "--vault-passphrase-file" => {
                if vault_passphrase_file.is_some() {
                    return Err("server takes --vault-passphrase-file at most once".into());
                }
                let value = args.get(index + 1).ok_or_else(|| {
                    "server --vault-passphrase-file takes an absolute path".to_owned()
                })?;
                let path = PathBuf::from(value);
                if value.is_empty() || value.contains('\0') || !path.is_absolute() {
                    return Err("server --vault-passphrase-file takes an absolute path".into());
                }
                vault_passphrase_file = Some(path);
                index += 2;
            }
            _ => {
                return Err(
                    "server takes [--provision-local] [--vault-passphrase-file PATH]".into(),
                );
            }
        }
    }
    Ok(ServerOptions {
        provisioning: provisioning.unwrap_or(LocalProvisioning::Manual),
        vault_passphrase_file,
    })
}
