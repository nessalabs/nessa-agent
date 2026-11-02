//! The machines named in the person's OpenSSH configuration that Devices
//! suggests connecting (`devices.sshHosts`).
//!
//! ```text
//! ~/.ssh/config ──▶ SshConfigSource::read (Include followed, bounded)
//!   ──▶ blocks: each `Host` line's patterns and the options under it
//!   ──▶ every alias named plainly (no `*`, `?` or `!`)
//!   ──▶ its HostName, User and Port: the first value any matching block gives
//!   ──▶ dropped: not a destination, a git server, already a configured host
//!   ──▶ SshHost, in the order the file names them, at most MAX_SUGGESTED
//! ```
//!
//! OpenSSH takes, for each option, the first value of any block whose
//! patterns match the name typed, so `Host *` blocks after an alias still
//! give it a `User`. `Match` blocks depend on things only `ssh` knows at
//! connection time (the local user, a command's exit); their options are
//! not taken for any alias, and an alias is never read out of one. A git
//! server is an alias whose user is `git` or whose host is a known forge:
//! it answers `git` commands, not a shell, so Nessa could not run there.
use nessa_sdk::domain::agent_execution::leases::SshDestination;

/// The most machines suggested at once: a page of pills.
pub(crate) const MAX_SUGGESTED: usize = 32;

/// The longest text a suggestion carries in any one field.
pub(crate) const MAX_FIELD_BYTES: usize = 253;

/// How many files an `Include` chain may read, the first included, and how
/// deep it may nest: OpenSSH's own nesting bound.
pub(crate) const MAX_FILES: usize = 32;
pub(crate) const MAX_INCLUDE_DEPTH: usize = 16;

/// The most bytes read across every file.
pub(crate) const SSH_CONFIG_BYTES: usize = 256 * 1024;

/// One machine an SSH configuration names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SshHost {
    /// The alias, as `ssh` is given it.
    pub(crate) host: SshDestination,
    /// Where it connects: `HostName`, when one is set.
    pub(crate) host_name: Option<String>,
    pub(crate) user: Option<String>,
    pub(crate) port: Option<u16>,
}

/// Reads the person's SSH configuration files. Implemented in
/// infrastructure over the file system; a path is as `Include` wrote it,
/// with `~` and relative paths resolved there.
pub(crate) trait SshConfigSource: Send + Sync {
    /// The main configuration's text; `None` when there is none.
    fn main(&self) -> Option<String>;
    /// The files one `Include` argument names, globs expanded, in the
    /// order `ssh` reads them, each with its text; at most `limit` of them.
    fn include(&self, pattern: &str, limit: usize) -> Vec<String>;
}

/// The hosts `source` names that Devices may suggest: every plainly named
/// alias that is a destination and not a git server, less `configured`.
pub(crate) fn suggestions(
    source: &dyn SshConfigSource,
    configured: &[SshDestination],
) -> Vec<SshHost> {
    let Some(main) = source.main() else {
        return Vec::new();
    };
    let mut read = Read {
        source,
        files: 1,
        bytes: main.len(),
    };
    let lines = read.lines(&main, 0);
    let blocks = blocks(&lines);
    let mut seen: Vec<String> = Vec::new();
    let mut hosts = Vec::new();
    for block in &blocks {
        let Patterns::Host(patterns) = &block.patterns else {
            continue;
        };
        for alias in patterns {
            if hosts.len() == MAX_SUGGESTED {
                return hosts;
            }
            if is_pattern(alias) || seen.iter().any(|each| each == alias) {
                continue;
            }
            seen.push(alias.clone());
            let Ok(host) = SshDestination::new(alias.clone()) else {
                continue;
            };
            if configured.contains(&host) {
                continue;
            }
            let found = resolve(&blocks, alias);
            let suggestion = SshHost {
                host,
                host_name: found.host_name.filter(|name| name.len() <= MAX_FIELD_BYTES),
                user: found.user.filter(|user| user.len() <= MAX_FIELD_BYTES),
                port: found.port,
            };
            if !git_server(&suggestion) {
                hosts.push(suggestion);
            }
        }
    }
    hosts
}

/// What a configured alias resolves to, as [`suggestions`] reads it: the
/// alias alone when the configuration does not name it.
pub(crate) fn resolved(source: &dyn SshConfigSource, host: &SshDestination) -> SshHost {
    let blocks = source
        .main()
        .map(|main| {
            let mut read = Read {
                source,
                files: 1,
                bytes: main.len(),
            };
            blocks(&read.lines(&main, 0))
        })
        .unwrap_or_default();
    let found = resolve(&blocks, host.as_str());
    SshHost {
        host: host.clone(),
        host_name: found.host_name.filter(|name| name.len() <= MAX_FIELD_BYTES),
        user: found.user.filter(|user| user.len() <= MAX_FIELD_BYTES),
        port: found.port,
    }
}

/// Forges whose SSH answers `git` and nothing else.
const GIT_FORGES: [&str; 13] = [
    "github.com",
    "ssh.github.com",
    "gist.github.com",
    "gitlab.com",
    "altssh.gitlab.com",
    "bitbucket.org",
    "altssh.bitbucket.org",
    "ssh.dev.azure.com",
    "vs-ssh.visualstudio.com",
    "codeberg.org",
    "git.sr.ht",
    "gitee.com",
    "heroku.com",
];

/// Host name prefixes a self-hosted git server goes by.
const GIT_PREFIXES: [&str; 4] = ["git.", "gitlab.", "gitea.", "forgejo."];

/// Whether `host` is a git server rather than a machine with a shell.
fn git_server(host: &SshHost) -> bool {
    if host.user.as_deref() == Some("git") {
        return true;
    }
    let name = host
        .host_name
        .as_deref()
        .unwrap_or(host.host.as_str())
        .trim_end_matches('.')
        .to_ascii_lowercase();
    // `git@github.com` as an alias is a git server too.
    let name = name.rsplit('@').next().unwrap_or(&name);
    GIT_FORGES.iter().any(|forge| {
        name == *forge
            || name
                .strip_suffix(forge)
                .is_some_and(|rest| rest.ends_with('.'))
    }) || GIT_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        || name.starts_with("git-codecommit.") && name.ends_with(".amazonaws.com")
}

/// Whether `name` is a pattern, not a name: it can match others.
fn is_pattern(name: &str) -> bool {
    name.contains(['*', '?', '!'])
}

/// The configuration read so far, bounded.
struct Read<'a> {
    source: &'a dyn SshConfigSource,
    files: usize,
    bytes: usize,
}

impl Read<'_> {
    /// `text`'s lines as keyword and arguments, with each `Include`
    /// replaced by the lines of the files it names, in place: what follows
    /// them in an included file is still under the block it was included in,
    /// as OpenSSH reads it.
    fn lines(&mut self, text: &str, depth: usize) -> Vec<(String, Vec<String>)> {
        let mut lines = Vec::new();
        for line in text.lines() {
            let Some((keyword, arguments)) = line_of(line) else {
                continue;
            };
            if keyword != "include" {
                lines.push((keyword, arguments));
                continue;
            }
            if depth >= MAX_INCLUDE_DEPTH {
                continue;
            }
            for pattern in arguments {
                let room = MAX_FILES.saturating_sub(self.files);
                for included in self.source.include(&pattern, room) {
                    if self.files >= MAX_FILES
                        || self.bytes.saturating_add(included.len()) > SSH_CONFIG_BYTES
                    {
                        return lines;
                    }
                    self.files += 1;
                    self.bytes += included.len();
                    lines.extend(self.lines(&included, depth + 1));
                }
            }
        }
        lines
    }
}

/// One line as its lowercase keyword and its arguments; `None` for a
/// blank line or a comment. A keyword may be followed by `=`; an argument
/// in double quotes may hold spaces.
fn line_of(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line
        .find(|character: char| character.is_whitespace() || character == '=')
        .unwrap_or(line.len());
    let keyword = line[..split].to_ascii_lowercase();
    let rest = line[split..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for character in rest.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            character if character.is_whitespace() && !quoted => {
                if any {
                    arguments.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            character => {
                current.push(character);
                any = true;
            }
        }
    }
    if any {
        arguments.push(current);
    }
    Some((keyword, arguments))
}

/// What a block applies to.
#[derive(Debug)]
enum Patterns {
    /// Lines before the first `Host` or `Match`: every host.
    All,
    /// A `Host` line's patterns.
    Host(Vec<String>),
    /// A `Match` line: decided only by `ssh` as it connects.
    Match,
}

#[derive(Debug)]
struct Block {
    patterns: Patterns,
    options: Vec<(String, String)>,
}

fn blocks(lines: &[(String, Vec<String>)]) -> Vec<Block> {
    let mut blocks = vec![Block {
        patterns: Patterns::All,
        options: Vec::new(),
    }];
    for (keyword, arguments) in lines {
        match keyword.as_str() {
            "host" => blocks.push(Block {
                patterns: Patterns::Host(arguments.clone()),
                options: Vec::new(),
            }),
            "match" => blocks.push(Block {
                patterns: Patterns::Match,
                options: Vec::new(),
            }),
            _ => {
                if let (Some(block), Some(value)) = (blocks.last_mut(), arguments.first()) {
                    block.options.push((keyword.clone(), value.clone()));
                }
            }
        }
    }
    blocks
}

#[derive(Default)]
struct Resolved {
    host_name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
}

/// The options `alias` takes: the first value of each, over the blocks
/// that match it, in order. `%h` in a `HostName` is the alias.
fn resolve(blocks: &[Block], alias: &str) -> Resolved {
    let mut found = Resolved::default();
    let mut port_seen = false;
    for block in blocks {
        let applies = match &block.patterns {
            Patterns::All => true,
            Patterns::Host(patterns) => host_matches(patterns, alias),
            Patterns::Match => false,
        };
        if !applies {
            continue;
        }
        for (keyword, value) in &block.options {
            match keyword.as_str() {
                "hostname" if found.host_name.is_none() => {
                    found.host_name = Some(value.replace("%h", alias));
                }
                "user" if found.user.is_none() => found.user = Some(value.clone()),
                "port" if !port_seen => {
                    port_seen = true;
                    found.port = value.parse().ok().filter(|port| *port != 0);
                }
                _ => {}
            }
        }
    }
    found
}

/// Whether a `Host` line's `patterns` match `name`: one positive pattern
/// matches and no negated one does.
fn host_matches(patterns: &[String], name: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        match pattern.strip_prefix('!') {
            Some(negated) => {
                if wildcard(negated, name) {
                    return false;
                }
            }
            None => matched |= wildcard(pattern, name),
        }
    }
    matched
}

/// OpenSSH's pattern match: `*` any run of characters, `?` any one,
/// without regard to case.
fn wildcard(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if let Some((at, from)) = star {
            p = at + 1;
            n = from + 1;
            star = Some((at, from + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|each| *each == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Files {
        main: Option<&'static str>,
        included: BTreeMap<&'static str, Vec<&'static str>>,
    }

    impl SshConfigSource for Files {
        fn main(&self) -> Option<String> {
            self.main.map(str::to_owned)
        }
        fn include(&self, pattern: &str, limit: usize) -> Vec<String> {
            self.included
                .get(pattern)
                .map(|files| files.iter().take(limit).map(|text| (*text).to_owned()).collect())
                .unwrap_or_default()
        }
    }

    fn files(main: &'static str) -> Files {
        Files {
            main: Some(main),
            included: BTreeMap::new(),
        }
    }

    fn named(hosts: &[SshHost]) -> Vec<&str> {
        hosts.iter().map(|host| host.host.as_str()).collect()
    }

    #[test]
    fn plain_aliases_are_suggested_with_what_they_resolve_to() {
        let config = files(
            "# my machines\n\
             Host box devbox\n  HostName 10.0.0.5\n  User ada\n  Port 2222\n\
             Host *.internal\n  User ops\n\
             Host build build-*\n  HostName build.example.net\n\
             Host *\n  User fallback\n  ServerAliveInterval 30\n",
        );
        let hosts = suggestions(&config, &[]);
        assert_eq!(named(&hosts), ["box", "devbox", "build"]);
        assert_eq!(
            hosts[0],
            SshHost {
                host: SshDestination::new("box").unwrap(),
                host_name: Some("10.0.0.5".into()),
                user: Some("ada".into()),
                port: Some(2222),
            }
        );
        // The first value wins: `Host *` gives build a user, not a host name.
        assert_eq!(hosts[2].host_name.as_deref(), Some("build.example.net"));
        assert_eq!(hosts[2].user.as_deref(), Some("fallback"));
    }

    #[test]
    fn git_servers_patterns_and_configured_hosts_are_left_out() {
        let config = files(
            "Host github.com\n  User git\n\
             Host gh\n  HostName ssh.github.com\n  Port 443\n\
             Host work-gitlab\n  HostName gitlab.com\n\
             Host forge\n  HostName code.example.com\n  User git\n\
             Host selfhosted\n  HostName git.example.com\n\
             Host azure\n  HostName ssh.dev.azure.com\n\
             Host codecommit\n  HostName git-codecommit.us-east-1.amazonaws.com\n\
             Host notgithub.community\n\
             Host taken\n\
             Host here\n\
             Host -oProxy\n\
             Host !nope* real\n\
             Match host *\n  User matched\n\
             Host after\n",
        );
        let taken = SshDestination::new("taken").unwrap();
        let hosts = suggestions(&config, &[taken]);
        // `notgithub.community` is not github.com; `here` is this machine's name.
        assert_eq!(named(&hosts), ["notgithub.community", "real", "after"]);
        // A Match block's options are taken for nothing.
        assert_eq!(hosts[2].user, None);
    }

    #[test]
    fn includes_are_read_in_place_and_bounded() {
        let mut config = files(
            "Include ~/.orbstack/ssh/config\nHost box\n  Include box.d/*\nHost later\n",
        );
        config
            .included
            .insert("~/.orbstack/ssh/config", vec!["Host orb\n  HostName 127.0.0.1\n"]);
        config.included.insert("box.d/*", vec!["  User ada\n", "Host inner\n"]);
        let hosts = suggestions(&config, &[]);
        assert_eq!(named(&hosts), ["orb", "box", "inner", "later"]);
        assert_eq!(hosts[1].user.as_deref(), Some("ada"));
        // An include of itself ends at the nesting bound.
        let mut looping = files("Include self\nHost a\n");
        looping.included.insert("self", vec!["Include self\nHost b\n"]);
        let hosts = suggestions(&looping, &[]);
        assert_eq!(named(&hosts), ["b", "a"]);
    }

    #[test]
    fn keyword_forms_quotes_and_case_are_read_as_ssh_reads_them() {
        let config = files(
            "HOST=\"spaced alias\" box\n\thostname=%h.lan\n  USER \"ada\"\n  port 0\n",
        );
        let hosts = suggestions(&config, &[]);
        // "spaced alias" is no destination; box is.
        assert_eq!(named(&hosts), ["box"]);
        assert_eq!(hosts[0].host_name.as_deref(), Some("box.lan"));
        assert_eq!(hosts[0].user.as_deref(), Some("ada"));
        assert_eq!(hosts[0].port, None);
    }

    #[test]
    fn no_file_suggests_nothing_and_many_aliases_stop_at_the_bound() {
        let none = Files {
            main: None,
            included: BTreeMap::new(),
        };
        assert!(suggestions(&none, &[]).is_empty());
        let many: String = (0..100).map(|index| format!("Host m{index}\n")).collect();
        let config = Files {
            main: Some(Box::leak(many.into_boxed_str())),
            included: BTreeMap::new(),
        };
        assert_eq!(suggestions(&config, &[]).len(), MAX_SUGGESTED);
    }

    #[test]
    fn a_configured_host_resolves_through_the_same_file() {
        let config = files("Host box\n  HostName 10.0.0.5\n  User ada\n");
        let host = SshDestination::new("box").unwrap();
        let found = resolved(&config, &host);
        assert_eq!(found.host_name.as_deref(), Some("10.0.0.5"));
        let unnamed = resolved(&config, &SshDestination::new("ada@other").unwrap());
        assert_eq!(unnamed.host_name, None);
    }

    #[test]
    fn wildcards_match_as_openssh_does() {
        assert!(wildcard("*.internal", "db.internal"));
        assert!(wildcard("b?x", "BOX"));
        assert!(wildcard("*a*b*", "xxaxxbxx"));
        assert!(!wildcard("*.internal", "internal"));
        assert!(host_matches(&["*".into(), "!secret".into()], "box"));
        assert!(!host_matches(&["*".into(), "!secret".into()], "secret"));
    }
}
