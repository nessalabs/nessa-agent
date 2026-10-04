use super::*;
use nessa_sync::replication::catalogue::{CataloguePass, ManifestRequest};
use nessa_sync::replication::domain::{Id, Scope};

fn scope() -> Scope {
    let id = Id::new("owner").unwrap();
    Scope::new(
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
        id,
        Id::new("1").unwrap(),
    )
}

#[test]
fn a_value_that_answers_another_operation_is_unverifiable() {
    let requested = ManifestRequest {
        pass: CataloguePass {
            scope: scope(),
            completed: 0,
            boundary: 1,
            cursor: None,
            generation: 1,
        },
        max_entries: 1,
    };
    assert_eq!(
        encode_value(
            "request",
            CatalogueReadOperation::Manifest(requested.clone()),
            CatalogueReadValue::Head {
                scope: requested.pass.scope,
                head: 1
            }
        ),
        Err(CatalogueReadErrorCode::Unverifiable)
    );
}
