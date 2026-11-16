//! Agent37 connection values and what an Agent37 VM is admitted under.
//! Validated here; no IO.
use super::Failure;
use nessa_vault::domain::Secret;

/// Who Nessa is on Agent37: the workspace's API key, and the SSH key this
/// install signs in to its VMs with. Agent37 registers SSH keys for the
/// whole workspace, so one key serves every VM. Both are secrets: redacted
/// and zeroizing.
#[derive(Clone)]
pub struct Agent37Account {
    pub key: Secret,
    pub ssh_private: Secret,
    pub ssh_public: String,
}
impl std::fmt::Debug for Agent37Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent37Account")
            .field("ssh_public", &self.ssh_public)
            .finish_non_exhaustive()
    }
}

/// The saved Agent37 connection: the account, and the pinned template
/// release new VMs are made from.
#[derive(Clone, Debug)]
pub struct Agent37Connection {
    pub account: Agent37Account,
    pub template: String,
}
impl Agent37Connection {
    pub fn new(
        key: String,
        ssh_private: Vec<u8>,
        ssh_public: String,
        template: String,
    ) -> Result<Self, Failure> {
        let key_ok =
            !key.is_empty() && key.len() <= 512 && key.bytes().all(|b| b.is_ascii_graphic());
        let public_ok = ssh_public.starts_with("ssh-ed25519 ")
            && ssh_public.len() <= 512
            && !ssh_public.contains(['\r', '\n']);
        if !key_ok
            || ssh_private.is_empty()
            || ssh_private.len() > 16 * 1024
            || !public_ok
            || !TemplateRelease::pinned(&template)
        {
            return Err(Failure::InvalidInput);
        }
        Ok(Self {
            account: Agent37Account {
                key: Secret::new(key.into_bytes()),
                ssh_private: Secret::new(ssh_private),
                ssh_public,
            },
            template,
        })
    }
}

/// What an Agent37 VM reported it runs: its template, the template's
/// revision and its image digest. An admitted VM's names the pinned release
/// it was made from and a digest, held to on every wake; a refused VM's is
/// whatever it reported, which may name no digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateRelease {
    template: String,
    revision: Option<u64>,
    digest: Option<String>,
}
impl TemplateRelease {
    /// Whether `template` names one release (`name@revision` or a system
    /// template's `name@tag`).
    pub fn pinned(template: &str) -> bool {
        template.len() <= 128
            && template.matches('@').count() == 1
            && template
                .split_once('@')
                .is_some_and(|(name, pin)| !name.is_empty() && !pin.is_empty())
            && template
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
    }
    /// # Errors
    /// [`Failure::InvalidInput`] for a template name outside Agent37's
    /// alphabet or a digest that is not one `sha256:` digest: what a caller
    /// read it from decides what that means (a corrupt record, a bad
    /// answer).
    pub fn new(
        template: String,
        revision: Option<u64>,
        digest: Option<String>,
    ) -> Result<Self, Failure> {
        let digest_ok = digest.as_deref().is_none_or(|digest| {
            digest
                .strip_prefix("sha256:")
                .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        });
        let named = !template.is_empty()
            && template.len() <= 128
            && template
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b));
        if !named || !digest_ok {
            return Err(Failure::InvalidInput);
        }
        Ok(Self {
            template,
            revision,
            digest,
        })
    }
    pub fn template(&self) -> &str {
        &self.template
    }
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }
    /// The image digest it reported, if it reported one. An admitted VM's
    /// always has one.
    pub fn digest(&self) -> Option<&str> {
        self.digest.as_deref()
    }
}
