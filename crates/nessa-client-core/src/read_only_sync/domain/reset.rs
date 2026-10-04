use nessa_sync::replication::domain::{Id, Scope};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResetError {
    Target,
    Generation,
}

/// Explicit local operator intent. Identity is reused only for exact retries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CacheReset {
    operation: Id,
    caller: Id,
    expected: Scope,
    generation: u64,
    replacement: Scope,
}

impl CacheReset {
    pub(crate) fn new(
        operation: Id,
        caller: Id,
        expected: Scope,
        generation: u64,
        replacement: Scope,
    ) -> Result<Self, ResetError> {
        if !expected.same_receiver_stream(&replacement) {
            return Err(ResetError::Target);
        }
        if generation == 0 || generation == u64::MAX {
            return Err(ResetError::Generation);
        }
        Ok(Self {
            operation,
            caller,
            expected,
            generation,
            replacement,
        })
    }
    pub(crate) fn operation(&self) -> &Id {
        &self.operation
    }
    pub(crate) fn caller(&self) -> &Id {
        &self.caller
    }
    pub(crate) fn expected(&self) -> &Scope {
        &self.expected
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn replacement(&self) -> &Scope {
        &self.replacement
    }
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/domain/reset.rs"]
mod tests;
