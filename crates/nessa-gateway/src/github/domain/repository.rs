//! The repository and read operation an agent may ask the gateway to perform.
//! No caller-supplied URL or path is retained.

/// One repository on github.com. Its parts are safe URL path segments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubRepository {
    owner: Box<str>,
    name: Box<str>,
}

/// The read the broker performs within one repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubReadKind {
    /// Repository metadata.
    Repository,
    /// One issue by number.
    Issue(u64),
    /// One pull request by number.
    PullRequest(u64),
}

/// Why an agent's repository read arguments were refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubReadInputError {
    /// The owner or repository name is not a GitHub path segment this broker accepts.
    Repository,
    /// An issue or pull request number must be positive.
    Number,
}

impl GithubRepository {
    /// Construct from two caller-supplied names, before any token or network access.
    pub fn new(owner: &str, name: &str) -> Result<Self, GithubReadInputError> {
        let owner_ok = !owner.is_empty()
            && owner.len() <= 39
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        let name_ok = !name.is_empty()
            && name.len() <= 100
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !owner_ok || !name_ok {
            return Err(GithubReadInputError::Repository);
        }
        Ok(Self {
            owner: owner.into(),
            name: name.into(),
        })
    }

    /// Owner name, preserved as given.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Repository name, preserved as given.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Path beneath the gateway's configured GitHub API base. Only the
    /// constructor's path segments can reach this method.
    pub fn path(&self, kind: GithubReadKind) -> Result<String, GithubReadInputError> {
        let root = format!("repos/{}/{}", self.owner, self.name);
        Ok(match kind {
            GithubReadKind::Repository => root,
            GithubReadKind::Issue(number) if number > 0 => format!("{root}/issues/{number}"),
            GithubReadKind::PullRequest(number) if number > 0 => {
                format!("{root}/pulls/{number}")
            }
            _ => return Err(GithubReadInputError::Number),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_paths_have_only_fixed_segments_and_positive_numbers() {
        let repo = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
        assert_eq!(
            repo.path(GithubReadKind::Repository).unwrap(),
            "repos/nessalabs/nessa-agent"
        );
        assert_eq!(
            repo.path(GithubReadKind::Issue(7)).unwrap(),
            "repos/nessalabs/nessa-agent/issues/7"
        );
        assert_eq!(
            repo.path(GithubReadKind::PullRequest(9)).unwrap(),
            "repos/nessalabs/nessa-agent/pulls/9"
        );
        assert_eq!(
            repo.path(GithubReadKind::Issue(0)),
            Err(GithubReadInputError::Number)
        );
        for (owner, name) in [
            ("", "repo"),
            ("a/b", "repo"),
            ("owner", ".."),
            ("owner", "repo/other"),
            ("owner", "repo?token=1"),
            ("owner", "%2e%2e"),
            ("owner", "repo\n"),
        ] {
            assert_eq!(
                GithubRepository::new(owner, name),
                Err(GithubReadInputError::Repository)
            );
        }
    }
}
