//! Run a dynamic app's own server, the way its app host does.
//!
//! ```text
//! <launcher>/credential.json   written by Nessa when the app was installed
//!                              (owner only): the app's credential and
//!                              where its gateway is published
//! start_server
//!   write <launcher>/server/   the package's verified files,
//!                              node_modules/@nessa/client (Nessa's own build)
//!                              and a bootstrap that ends with the host
//!   pick a free loopback port and a token for this launch
//!   run node|bun <bootstrap>   NESSA_APP_* and NESSA_STAGE/DATA_DIR/INSTANCE
//!   wait until GET /.nessa-ready on the port answers sha256(token) in hex
//!   (or the server ends: its log says why). The token is never sent, so a
//!   program that took the port first cannot pass for the app's server.
//! drop RunningServer            closes its stdin, then kills what is left
//! ```
//!
//! The server signs in to Nessa with the app's own credential through
//! `@nessa/client`, which finds the gateway from its published endpoint, as
//! any Nessa client on this computer does, so a gateway restart on another
//! port does not strand it. What it may do there is what the owner approved
//! at install, and nothing of the owner's. On this computer it is an
//! ordinary program the owner chose to install: it runs as them.
//!
//! The page reaches it only on 127.0.0.1, with the launch's token, which no
//! other page holds.
//!
//! What a server must do: listen on `NESSA_APP_HOST`:`NESSA_APP_PORT`,
//! answer `GET /.nessa-ready` with sha256(`NESSA_APP_TOKEN`) in hex, and
//! refuse any other request without the `x-nessa-app-token` header. The
//! starter (`server-starter/main.mjs`) does all three.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AppServer, ServerRuntime, VerifiedPackage};

/// The app's credential record in its launcher folder.
pub const APP_CREDENTIAL: &str = "credential.json";
/// The most a credential record may hold.
pub const MAX_APP_CREDENTIAL_BYTES: usize = 32 * 1024;
/// The folder a server runs in, inside the launcher folder.
pub const SERVER_FOLDER: &str = "server";
/// What a server wrote, beside its folder, for when it does not start.
pub const SERVER_LOG: &str = "server.log";

const BOOTSTRAP: &str = ".nessa-bootstrap.mjs";
const CLIENT_PACKAGE: &str = r#"{"name":"@nessa/client","type":"module","main":"./index.mjs","exports":{".":"./index.mjs"}}"#;
/// How long a server has to answer on its port.
const START_WAIT: Duration = Duration::from_secs(20);
/// How long a server has to end once its stdin closes.
const STOP_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);
/// The path a server proves it is this launch's on.
pub const READY_PATH: &str = "/.nessa-ready";
/// The most of a readiness answer read.
const READY_BYTES: u64 = 4 * 1024;
/// The last of the log a refusal quotes.
const LOG_TAIL: u64 = 2 * 1024;

/// What Nessa hands a dynamic app at install: its credential, and where its
/// gateway publishes its endpoint.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppCredentialRecord {
    /// The credential `apps.issueCredential` gave, secret.
    pub credential: String,
    /// The Nessa stage the gateway runs as (`prod`, `dev`, ...).
    pub stage: String,
    /// The gateway's data root, when it is not the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// The gateway's instance, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// When the credential stops working, in Unix seconds, as the gateway
    /// said when it issued it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

impl AppCredentialRecord {
    /// The record's bytes, as written to [`APP_CREDENTIAL`].
    ///
    /// # Errors
    /// Never in practice: the record is plain strings.
    pub fn encode(&self) -> Result<Vec<u8>, ServerRefusal> {
        serde_json::to_vec(self).map_err(|_| ServerRefusal::Credential)
    }

    /// Read a record, refusing one too large or not exactly this shape.
    ///
    /// # Errors
    /// [`ServerRefusal::Credential`] for anything but a whole record.
    pub fn decode(bytes: &[u8]) -> Result<Self, ServerRefusal> {
        if bytes.len() > MAX_APP_CREDENTIAL_BYTES {
            return Err(ServerRefusal::Credential);
        }
        let record: Self = serde_json::from_slice(bytes).map_err(|_| ServerRefusal::Credential)?;
        let segment = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        };
        if record.credential.is_empty()
            || record.credential.chars().any(char::is_control)
            || !segment(&record.stage)
            || record
                .instance
                .as_deref()
                .is_some_and(|value| !segment(value))
            || record
                .data_dir
                .as_deref()
                .is_some_and(|value| !Path::new(value).is_absolute())
        {
            return Err(ServerRefusal::Credential);
        }
        Ok(record)
    }

    /// Whether the credential still works `margin` seconds after `now`: a
    /// record that names no expiry is taken at its word.
    pub fn lasts_past(&self, now: u64, margin: u64) -> bool {
        self.expires_at
            .is_none_or(|at| at > now.saturating_add(margin))
    }
}

/// Why a server did not start.
#[derive(Debug, PartialEq, Eq)]
pub enum ServerRefusal {
    /// The package declares no server.
    NotDynamic,
    /// No credential record, or not a whole one: the app was installed
    /// without one, or it was taken back.
    Credential,
    /// Neither the runtime it names nor any usual place holds one.
    NoRuntime(ServerRuntime),
    /// Its files could not be written.
    Files(String),
    /// It could not be run.
    Spawn(String),
    /// It ended, or did not answer in time; the end of its log.
    DidNotStart(String),
}

impl std::fmt::Display for ServerRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDynamic => formatter.write_str("the app has no server"),
            Self::Credential => formatter
                .write_str("the app has no credential: install it again from Nessa's Settings"),
            Self::NoRuntime(runtime) => write!(
                formatter,
                "the app's server needs {}, which is not installed",
                runtime.as_str()
            ),
            Self::Files(error) => write!(formatter, "could not write the app's server: {error}"),
            Self::Spawn(error) => write!(formatter, "could not run the app's server: {error}"),
            Self::DidNotStart(log) => write!(formatter, "the app's server did not start: {log}"),
        }
    }
}

/// A started server. Dropping it ends it.
#[derive(Debug)]
pub struct RunningServer {
    child: Child,
    stdin: Option<ChildStdin>,
    port: u16,
    token: String,
}

impl RunningServer {
    /// The loopback port it answers on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The origin the page reaches it at.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// This launch's token: the page sends it, and the server checks it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Whether it is still running.
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        // Its bootstrap ends it when stdin closes; anything left is killed.
        drop(self.stdin.take());
        let deadline = Instant::now() + STOP_WAIT;
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `package`'s server from its launcher `folder`, with `client` as its
/// `@nessa/client`. `runtime` names the program to run it with; None looks
/// for the one the package names.
///
/// # Errors
/// A [`ServerRefusal`] saying why it is not running.
pub fn start_server(
    package: &VerifiedPackage,
    folder: &Path,
    client: &[u8],
    runtime: Option<PathBuf>,
) -> Result<RunningServer, ServerRefusal> {
    let server = package
        .manifest()
        .server()
        .ok_or(ServerRefusal::NotDynamic)?;
    let record = read_credential(folder)?;
    let program = match runtime {
        Some(program) => program,
        None => find_runtime(server.runtime()).ok_or(ServerRefusal::NoRuntime(server.runtime()))?,
    };
    let root = folder.join(SERVER_FOLDER);
    write_tree(package, server, &root, client)
        .map_err(|error| ServerRefusal::Files(error.to_string()))?;
    let port = free_port().map_err(|error| ServerRefusal::Spawn(error.to_string()))?;
    let token = token().map_err(ServerRefusal::Spawn)?;
    let log_path = folder.join(SERVER_LOG);
    let log = private_log(&log_path).map_err(|error| ServerRefusal::Files(error.to_string()))?;
    let errors = log
        .try_clone()
        .map_err(|error| ServerRefusal::Files(error.to_string()))?;

    let mut command = Command::new(&program);
    // Nothing of Nessa's own environment reaches it but what is set here.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("NESSA_") {
            command.env_remove(name);
        }
    }
    command
        .arg(BOOTSTRAP)
        .current_dir(&root)
        .env("NESSA_APP_ID", package.manifest().app_id().as_str())
        .env("NESSA_APP_HOST", Ipv4Addr::LOCALHOST.to_string())
        .env("NESSA_APP_PORT", port.to_string())
        .env("NESSA_APP_TOKEN", &token)
        .env("NESSA_APP_CREDENTIAL", &record.credential)
        .env("NESSA_STAGE", &record.stage)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    if let Some(dir) = &record.data_dir {
        command.env("NESSA_DATA_DIR", dir);
    }
    if let Some(instance) = &record.instance {
        command.env("NESSA_INSTANCE", instance);
    }
    let mut child = command
        .spawn()
        .map_err(|error| ServerRefusal::Spawn(format!("{}: {error}", program.display())))?;
    let stdin = child.stdin.take();
    let mut running = RunningServer {
        child,
        stdin,
        port,
        token,
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let proof = ready_proof(&running.token);
    let deadline = Instant::now() + START_WAIT;
    loop {
        if !running.running() {
            return Err(ServerRefusal::DidNotStart(log_tail(&log_path)));
        }
        if answers_ready(&address, &proof) {
            return Ok(running);
        }
        if Instant::now() >= deadline {
            return Err(ServerRefusal::DidNotStart(format!(
                "no answer on its port after {} s. {}",
                START_WAIT.as_secs(),
                log_tail(&log_path)
            )));
        }
        thread::sleep(POLL);
    }
}

/// What a server answers on [`READY_PATH`] for `token`: sha256 in hex.
pub fn ready_proof(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Whether the server at `address` answers [`READY_PATH`] with `proof`.
fn answers_ready(address: &SocketAddr, proof: &str) -> bool {
    use std::io::Write;
    let Ok(mut stream) = TcpStream::connect_timeout(address, POLL) else {
        return false;
    };
    let wait = Some(Duration::from_secs(1));
    if stream.set_read_timeout(wait).is_err() || stream.set_write_timeout(wait).is_err() {
        return false;
    }
    // HTTP/1.0, so no server answers in chunks: the body is the proof alone.
    let request = format!("GET {READY_PATH} HTTP/1.0\r\nHost: {address}\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut answer = Vec::new();
    let _ = stream.take(READY_BYTES).read_to_end(&mut answer);
    let answer = String::from_utf8_lossy(&answer);
    let Some((head, body)) = answer.split_once("\r\n\r\n") else {
        return false;
    };
    head.lines()
        .next()
        .is_some_and(|status| status.split(' ').nth(1) == Some("200"))
        && body.trim() == proof
}

/// The server's log, this user's alone: what it prints may hold secrets.
fn private_log(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let file = options.open(path)?;
        // A log left by an older build keeps its mode on open.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    // On Windows the launcher folder is in the user's own profile, whose
    // ACL already keeps other users out.
    #[cfg(not(unix))]
    options.open(path)
}

fn read_credential(folder: &Path) -> Result<AppCredentialRecord, ServerRefusal> {
    let file = File::open(folder.join(APP_CREDENTIAL)).map_err(|_| ServerRefusal::Credential)?;
    let mut bytes = Vec::new();
    file.take(MAX_APP_CREDENTIAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ServerRefusal::Credential)?;
    AppCredentialRecord::decode(&bytes)
}

/// Write the server's folder afresh: the package's files, then Nessa's
/// client and the bootstrap, so the package can replace neither.
fn write_tree(
    package: &VerifiedPackage,
    server: &AppServer,
    root: &Path,
    client: &[u8],
) -> io::Result<()> {
    match fs::remove_dir_all(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut files: BTreeMap<PathBuf, Vec<u8>> = package
        .assets()
        .map(|(path, bytes)| (root.join(path), bytes.to_vec()))
        .collect();
    let client_folder = root.join("node_modules").join("@nessa").join("client");
    files.insert(
        client_folder.join("package.json"),
        CLIENT_PACKAGE.as_bytes().to_vec(),
    );
    files.insert(client_folder.join("index.mjs"), client.to_vec());
    files.insert(root.join(BOOTSTRAP), bootstrap(server.entry()).into_bytes());
    for (path, bytes) in files {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    Ok(())
}

/// Ends the server when its host does: the host holds its stdin, and a
/// host that ends for any reason closes it. `@nessa/client` needs a global
/// WebSocket (Node 22 or later, or Bun), so an older runtime says so.
fn bootstrap(entry: &str) -> String {
    let entry = serde_json::to_string(&format!("./{entry}")).unwrap_or_default();
    format!(
        "process.stdin.on(\"end\", () => process.exit(0))\n\
         process.stdin.on(\"error\", () => process.exit(0))\n\
         process.stdin.resume()\n\
         if (typeof globalThis.WebSocket !== \"function\") {{\n\
         \x20 console.error(`Nessa apps need Node.js 22 or later, or Bun; this is ${{process.version}}`)\n\
         \x20 process.exit(1)\n\
         }}\n\
         await import({entry})\n"
    )
}

fn free_port() -> io::Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

fn token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(hex::encode(bytes))
}

fn log_tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(LOG_TAIL)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

/// Where `runtime` is on this computer: on PATH, or where its installers
/// usually put it. An app opened from Finder or a menu gets the system's
/// short PATH, not the one a terminal has.
pub fn find_runtime(runtime: ServerRuntime) -> Option<PathBuf> {
    let name = format!("{}{}", runtime.as_str(), std::env::consts::EXE_SUFFIX);
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut places: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = &home {
        places.push(home.join(".bun").join("bin"));
        places.push(home.join(".volta").join("bin"));
        places.push(home.join(".local").join("bin"));
        places.push(home.join(".local").join("share").join("mise").join("shims"));
        places.push(home.join(".asdf").join("shims"));
        // nvm keeps one folder per version (`v22.11.0`): the newest first.
        if let Ok(versions) = fs::read_dir(home.join(".nvm").join("versions").join("node")) {
            let mut found: Vec<(Vec<u64>, PathBuf)> = versions
                .filter_map(Result::ok)
                .map(|entry| {
                    (
                        nvm_version(&entry.file_name().to_string_lossy()),
                        entry.path(),
                    )
                })
                .collect();
            found.sort();
            places.extend(found.into_iter().rev().map(|(_, path)| path.join("bin")));
        }
    }
    if cfg!(windows) {
        if let Some(programs) = std::env::var_os("ProgramFiles") {
            places.push(PathBuf::from(programs).join("nodejs"));
        }
    } else {
        places.push(PathBuf::from("/opt/homebrew/bin"));
        places.push(PathBuf::from("/usr/local/bin"));
        places.push(PathBuf::from("/usr/bin"));
    }
    places
        .into_iter()
        .filter(|place| place.is_absolute())
        .map(|place| place.join(&name))
        .find(|candidate| candidate.is_file())
}

/// An nvm folder's version as numbers, so `v22` sorts after `v9`.
fn nvm_version(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> AppCredentialRecord {
        AppCredentialRecord {
            credential: "secret".into(),
            stage: "dev".into(),
            data_dir: Some(if cfg!(windows) { "C:\\data" } else { "/data" }.into()),
            instance: Some("ci-1".into()),
            expires_at: Some(1_000),
        }
    }

    #[test]
    fn a_credential_record_reads_back_only_whole() {
        let bytes = record().encode().unwrap();
        assert_eq!(AppCredentialRecord::decode(&bytes).unwrap(), record());
        for broken in [
            AppCredentialRecord {
                stage: "../x".into(),
                ..record()
            },
            AppCredentialRecord {
                instance: Some("a/b".into()),
                ..record()
            },
            AppCredentialRecord {
                data_dir: Some("relative".into()),
                ..record()
            },
            AppCredentialRecord {
                credential: String::new(),
                ..record()
            },
            AppCredentialRecord {
                credential: "a\nb".into(),
                ..record()
            },
        ] {
            let bytes = serde_json::to_vec(&broken).unwrap();
            assert_eq!(
                AppCredentialRecord::decode(&bytes),
                Err(ServerRefusal::Credential)
            );
        }
        assert_eq!(
            AppCredentialRecord::decode(br#"{"credential":"a","stage":"dev","url":"ws://x"}"#),
            Err(ServerRefusal::Credential)
        );
        assert_eq!(
            AppCredentialRecord::decode(&vec![b' '; MAX_APP_CREDENTIAL_BYTES + 1]),
            Err(ServerRefusal::Credential)
        );
    }

    #[test]
    fn a_record_lasts_until_its_expiry_less_the_margin() {
        assert!(record().lasts_past(900, 50));
        assert!(!record().lasts_past(950, 50));
        assert!(AppCredentialRecord {
            expires_at: None,
            ..record()
        }
        .lasts_past(u64::MAX, 50));
    }

    #[test]
    fn nvm_versions_sort_as_numbers() {
        let mut names = ["v9.11.2", "v22.11.0", "v20.18.1"].map(nvm_version);
        names.sort();
        assert_eq!(names.last(), Some(&nvm_version("v22.11.0")));
    }

    #[test]
    fn only_the_launch_holding_the_token_answers_ready() {
        use std::io::Write;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let proof = ready_proof("token");
        let serve = |body: String| {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = [0_u8; 512];
            let _ = stream.read(&mut head);
            assert!(!String::from_utf8_lossy(&head).contains("token"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        };
        thread::scope(|scope| {
            scope.spawn(|| serve(proof.clone()));
            assert!(answers_ready(&address, &proof));
        });
        thread::scope(|scope| {
            scope.spawn(|| serve(ready_proof("another")));
            assert!(!answers_ready(&address, &proof));
        });
    }

    #[test]
    fn the_bootstrap_imports_the_declared_entry_as_a_string() {
        let script = bootstrap("server/a\"b.mjs");
        assert!(
            script.ends_with("await import(\"./server/a\\\"b.mjs\")\n"),
            "{script}"
        );
    }
}
