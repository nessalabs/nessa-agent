use super::*;

#[test]
fn single_pin_cli_preserves_capability_and_horizon_causes_losslessly() {
    assert_eq!(
        core_failure(&SyncError::Source(SourceError::Incompatible(
            CapabilityError::MissingReader(u64::MAX)
        ))),
        json!({"code":"source","cause":{"code":"incompatible","cause":{"code":"missingReader","required":u64::MAX.to_string()}}})
    );
    assert_eq!(
        core_failure(&SyncError::Validation(ValidationError::HeadBehindProgress)),
        json!({"code":"validation","cause":{"code":"headBehindProgress"}})
    );
    assert_eq!(
        catalogue_failure(&CatalogueError::Store(
            CatalogueStoreError::HorizonRegressed {
                previous: u64::MAX,
                received: 30,
            }
        )),
        json!({"code":"store","cause":{"code":"horizonRegressed","previous":u64::MAX.to_string(),"received":"30"}})
    );
    assert_eq!(
        catalogue_failure(&CatalogueError::Source(
            CatalogueSourceError::HorizonRequired
        )),
        json!({"code":"source","cause":{"code":"horizonRequired"}})
    );
    assert_eq!(
        catalogue_failure(&CatalogueError::Source(CatalogueSourceError::Unavailable)),
        json!({"code":"source","cause":{"code":"unavailable"}})
    );
}
