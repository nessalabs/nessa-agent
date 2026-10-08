//! The grant each passive read asks Cedar for.
//!
//! `action_for_method` is generated from `protocol/product/manifest.json`.
//! Conversation admission asks this adapter through `PassiveReadGrants` and
//! does not import the generated mapping itself.

use crate::conversation::application::{PassiveRead, PassiveReadGrants};
use nessa_protocol::product::generated::{action_for_method, product_method};

/// Manifest grants for the five passive reads.
pub(crate) struct PublishedPassiveReadGrants;

/// One process-wide adapter. The mapping is static.
pub(crate) static PUBLISHED_PASSIVE_READ_GRANTS: PublishedPassiveReadGrants =
    PublishedPassiveReadGrants;

impl PassiveReadGrants for PublishedPassiveReadGrants {
    fn grant(&self, read: PassiveRead) -> Option<&'static str> {
        let method = match read {
            PassiveRead::RecordHead => product_method::CONVERSATION_RECORDS_HEAD,
            PassiveRead::RecordPage => product_method::CONVERSATION_RECORDS_PAGE,
            PassiveRead::CatalogueHead => product_method::CONVERSATION_CATALOGUE_HEAD,
            PassiveRead::CatalogueManifest => product_method::CONVERSATION_CATALOGUE_MANIFEST,
            PassiveRead::CatalogueResolve => product_method::CONVERSATION_CATALOGUE_RESOLVE,
        };
        action_for_method(method)
    }
}
