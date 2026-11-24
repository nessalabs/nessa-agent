use super::*;
use nessa_sync::replication::domain::CapabilityError;

#[test]
fn single_pin_record_incompatibility_is_an_unverifiable_v1_read() {
    assert_eq!(
        error_code(RecordReadError::Incompatible(
            CapabilityError::MissingReader(4)
        )),
        RecordReadErrorCode::Unverifiable
    );
    assert_eq!(
        error_code(RecordReadError::Admission(ReadRefusal::Forbidden)),
        RecordReadErrorCode::Forbidden
    );
    assert_eq!(
        error_code(RecordReadError::Admission(ReadRefusal::StaleEpoch)),
        RecordReadErrorCode::StaleEpoch
    );
}
