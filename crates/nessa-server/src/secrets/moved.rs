//! A secret that moved from one entry to another: the first read after the
//! move finds it under its old name, writes it under the new one and
//! deletes the old, so one entry holds it from then on.
use super::{SecretStore, SecretUnavailable};
use std::sync::Arc;
use zeroize::Zeroizing;

pub struct MovedSecret {
    now: Arc<dyn SecretStore>,
    before: Arc<dyn SecretStore>,
}

impl MovedSecret {
    /// The secret kept in `now`, which an older build kept in `before`.
    pub fn new(now: Arc<dyn SecretStore>, before: Arc<dyn SecretStore>) -> Self {
        Self { now, before }
    }
}

impl SecretStore for MovedSecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        if let Some(value) = self.now.read()? {
            return Ok(Some(value));
        }
        let Some(value) = self.before.read()? else {
            return Ok(None);
        };
        // Written under the new name before the old is deleted: a failure
        // between the two leaves it in both, never in neither.
        self.now.write(&value)?;
        self.before.delete()?;
        Ok(Some(value))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        self.now.write(bytes)?;
        // An old copy left behind would come back after a delete.
        self.forget_before()
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        self.forget_before()?;
        self.now.delete()
    }
}

impl MovedSecret {
    /// Delete the old entry if there is one: each delete is recorded in the
    /// vault's evidence, so an absent one is not deleted again and again.
    fn forget_before(&self) -> Result<(), SecretUnavailable> {
        match self.before.read()? {
            Some(_) => self.before.delete(),
            None => Ok(()),
        }
    }
}
