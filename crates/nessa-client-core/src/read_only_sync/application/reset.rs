//! Local operator reset returns the cache owner's original durable receipt.
use super::{CacheError, CatalogueResetReceipt, ResetReceipt};
use crate::read_only_sync::domain::CacheReset;

pub(crate) trait CacheResets {
    fn reset_records(&mut self, request: &CacheReset) -> Result<ResetReceipt, CacheError>;
    fn reset_catalogue(
        &mut self,
        request: &CacheReset,
    ) -> Result<CatalogueResetReceipt, CacheError>;
}
