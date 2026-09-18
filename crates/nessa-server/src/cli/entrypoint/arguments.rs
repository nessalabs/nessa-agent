use std::path::PathBuf;

pub const HELP: &str = "Nessa\n\n  nessa server [--provision-local]\n  nessa auth init --local [--owner-token-file PATH]\n  nessa auth token [--local] [--ttl 12h|7d|30m|60s | --no-expiry] [--credential-file PATH]\n  nessa doctor [--local] [--credential-file PATH]\n  nessa auth provision-surface --local --surface-id NAME [--grants ACTIONS]\n  nessa auth recover-owner --local --owner-token-file PATH\n\nLocal is the current backend. Cloud auth is not implemented.\n`server --provision-local` creates the owner and panel credentials of the selected\nnamespace when they are absent, then serves; it never replaces existing ones.\nWithout it, `nessa server` only serves what the offline auth commands provisioned.\nNESSA_HOST, NESSA_PORT, NESSA_STAGE, NESSA_DATA_DIR and NESSA_INSTANCE select the local gateway.\nToken defaults to no expiry (capped by issuer expiry) and prints only the secret to stdout; pipe it to pbcopy.\n";

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
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "--cloud") {
        return Err("Cloud authentication is not implemented yet; use --local".into());
    }
    if args == ["server"] {
        return Ok(Command::Server(LocalProvisioning::Manual));
    }
    if args == ["server", "--provision-local"] {
        return Ok(Command::Server(LocalProvisioning::Automatic));
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
