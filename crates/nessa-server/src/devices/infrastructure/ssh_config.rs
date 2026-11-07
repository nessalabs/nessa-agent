//! [`SshConfigSource`] over the file system: `~/.ssh/config` and the files
//! its `Include` lines name, each read up to the configuration's bound.
//!
//! As OpenSSH resolves them, `~/` is the home directory and a relative path
//! is under `~/.ssh`. A glob's `*` and `?` are expanded in the path's last
//! part only, the files sorted by name; what `ssh` would find through a
//! glob in a directory part is not read.
use crate::devices::application::{SshConfigSource, SSH_CONFIG_BYTES};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// The person's SSH configuration, under `home`.
pub(crate) struct HomeSshConfig {
    home: PathBuf,
}

impl HomeSshConfig {
    pub(crate) fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None if Path::new(path).is_absolute() => PathBuf::from(path),
            None => self.home.join(".ssh").join(path),
        }
    }
}

/// `path`'s text when it is a file of at most the configuration's bound.
fn text(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(SSH_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > SSH_CONFIG_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

impl SshConfigSource for HomeSshConfig {
    fn main(&self) -> Option<String> {
        text(&self.home.join(".ssh").join("config"))
    }

    fn include(&self, pattern: &str, limit: usize) -> Vec<String> {
        let path = self.resolve(pattern);
        let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
            return Vec::new();
        };
        let name = name.to_string_lossy();
        if !name.contains(['*', '?']) {
            return text(&path).into_iter().take(limit).collect();
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|each| !each.starts_with('.') && glob(&name, each))
            .collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|each| text(&directory.join(each)))
            .take(limit)
            .collect()
    }
}

/// A file name glob: `*` any run, `?` any one character.
fn glob(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
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

    #[test]
    fn reads_the_config_and_its_includes_from_home() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(ssh.join("conf.d")).unwrap();
        fs::write(ssh.join("config"), "Include conf.d/*.conf\nHost box\n").unwrap();
        fs::write(ssh.join("conf.d/b.conf"), "Host b\n").unwrap();
        fs::write(ssh.join("conf.d/a.conf"), "Host a\n").unwrap();
        fs::write(ssh.join("conf.d/skip.txt"), "Host skip\n").unwrap();
        fs::create_dir_all(home.path().join(".orbstack/ssh")).unwrap();
        fs::write(home.path().join(".orbstack/ssh/config"), "Host orb\n").unwrap();
        let source = HomeSshConfig::new(home.path().to_path_buf());
        assert_eq!(
            source.main().as_deref(),
            Some("Include conf.d/*.conf\nHost box\n")
        );
        assert_eq!(source.include("conf.d/*.conf", 8), ["Host a\n", "Host b\n"]);
        assert_eq!(source.include("conf.d/*.conf", 1), ["Host a\n"]);
        assert_eq!(source.include("~/.orbstack/ssh/config", 8), ["Host orb\n"]);
        assert!(source.include("missing", 8).is_empty());
    }

    #[test]
    fn no_config_or_one_past_the_bound_reads_as_none() {
        let home = tempfile::tempdir().unwrap();
        let source = HomeSshConfig::new(home.path().to_path_buf());
        assert_eq!(source.main(), None);
        fs::create_dir_all(home.path().join(".ssh")).unwrap();
        fs::write(
            home.path().join(".ssh/config"),
            vec![b'#'; SSH_CONFIG_BYTES + 1],
        )
        .unwrap();
        assert_eq!(source.main(), None);
    }
}
