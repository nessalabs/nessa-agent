//! Cryptographic session availability for cached sign-in state and provider opens.
use crate::github::application::GithubAccess;
use nessa_vault::application::Vault;
use std::sync::Arc;
pub struct GithubVaultAccess(pub Arc<Vault>);
impl GithubAccess for GithubVaultAccess {
    fn generation(&self) -> Option<u64> {
        self.0.unlocked_generation()
    }
}
