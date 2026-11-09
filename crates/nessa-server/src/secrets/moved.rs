//! A secret that moved from one entry to another: the first read after the
//! move finds it under its old name, writes it under the new one and
//! deletes the old, so one entry holds it from then on.
use super::{SecretStore, SecretUnavailable};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// Share one per secret: its lock keeps a read that moves the old entry
/// from racing a write or delete, which would bring the old value back.
pub struct MovedSecret {
    now: Arc<dyn SecretStore>,
    before: Arc<dyn SecretStore>,
    steps: Mutex<()>,
}

impl MovedSecret {
    /// The secret kept in `now`, which an older build kept in `before`.
    pub fn new(now: Arc<dyn SecretStore>, before: Arc<dyn SecretStore>) -> Self {
        Self {
            now,
            before,
            steps: Mutex::new(()),
        }
    }
}

impl SecretStore for MovedSecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        let _steps = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _steps = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.now.write(bytes)?;
        // The new entry holds it now, and is read first: an old copy that
        // cannot be deleted yet is harmless until a delete, which deletes
        // it first.
        if self.forget_before().is_err() {
            tracing::warn!("an older copy of a saved secret could not be deleted yet");
        }
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        let _steps = self
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory {
        value: Mutex<Option<Vec<u8>>>,
        refuse_delete: bool,
    }
    impl SecretStore for Memory {
        fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
            Ok(self.value.lock().unwrap().clone().map(Zeroizing::new))
        }
        fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
            *self.value.lock().unwrap() = Some(bytes.to_vec());
            Ok(())
        }
        fn delete(&self) -> Result<(), SecretUnavailable> {
            if self.refuse_delete {
                return Err(SecretUnavailable::Unavailable);
            }
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }
    fn held(store: &Memory) -> Option<Vec<u8>> {
        store.value.lock().unwrap().clone()
    }

    #[test]
    fn an_old_entry_is_moved_once_and_a_write_or_delete_never_brings_it_back() {
        let now = Arc::new(Memory::default());
        let before = Arc::new(Memory::default());
        before.write(b"old").unwrap();
        let moved = MovedSecret::new(now.clone(), before.clone());
        assert_eq!(
            moved.read().unwrap().as_deref().map(Vec::as_slice),
            Some(&b"old"[..])
        );
        assert_eq!(held(&now), Some(b"old".to_vec()));
        assert_eq!(held(&before), None);
        before.write(b"stale").unwrap();
        moved.write(b"new").unwrap();
        assert_eq!(held(&before), None);
        before.write(b"stale").unwrap();
        moved.delete().unwrap();
        assert_eq!(moved.read().unwrap(), None);
    }

    #[test]
    fn a_write_holds_once_the_new_entry_does() {
        let now = Arc::new(Memory::default());
        let before = Arc::new(Memory {
            refuse_delete: true,
            ..Memory::default()
        });
        before.write(b"old").unwrap();
        let moved = MovedSecret::new(now.clone(), before.clone());
        // The old copy cannot go yet, but the new value is the one read.
        moved.write(b"new").unwrap();
        assert_eq!(
            moved.read().unwrap().as_deref().map(Vec::as_slice),
            Some(&b"new"[..])
        );
        // A delete that cannot remove the old copy deletes nothing, so the
        // old value cannot come back in its place.
        assert_eq!(moved.delete(), Err(SecretUnavailable::Unavailable));
        assert_eq!(held(&now), Some(b"new".to_vec()));
    }
}
