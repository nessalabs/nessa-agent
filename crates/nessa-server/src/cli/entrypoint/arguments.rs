use std::path::PathBuf;

use crate::agent_install::domain::AgentName;

pub const HELP: &str = "\
Nessa

  nessa server
  nessa auth init --local [--owner-token-file PATH]
  nessa auth token [--local] [--ttl 12h|7d|30m|60s | --no-expiry] [--credential-file PATH]
  nessa doctor [--local] [--credential-file PATH]
  nessa install-agent NAME
  nessa auth provision-surface --local --surface-id NAME [--grants ACTIONS]
  nessa auth recover-owner --local --owner-token-file PATH

install-agent downloads the release of an agent's own runtime that Nessa has
tested, verifies it against a compiled-in digest, and reports what it installed
as JSON on stdout. Installing one already present downloads nothing.

Local is the current backend. Cloud auth is not implemented.
NESSA_HOST, NESSA_PORT, NESSA_STAGE, NESSA_DATA_DIR and NESSA_INSTANCE select the local gateway.
Token defaults to no expiry (capped by issuer expiry) and prints only the secret to stdout; pipe it to pbcopy.
";

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Server,
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
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "--cloud") {
        return Err("Cloud authentication is not implemented yet; use --local".into());
    }
    if args == ["server"] {
        return Ok(Command::Server);
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
