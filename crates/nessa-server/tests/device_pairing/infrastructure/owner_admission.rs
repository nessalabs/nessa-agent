//! Owner admission tells a full gateway from a closed one (row S19).

use super::*;

#[tokio::test]
async fn full_admission_is_not_reported_as_shutdown() {
    let admission = OwnerAdmission::new();
    let leases: Vec<_> = (0..OWNER_COMMAND_CAPACITY)
        .map(|_| admission.admit().expect("capacity remains"))
        .collect();
    assert_eq!(admission.admit().err(), Some(OwnerAdmissionRefusal::Full));
    drop(leases);
    let lease = admission.admit().expect("a returned lease admits again");
    admission.close();
    assert_eq!(admission.admit().err(), Some(OwnerAdmissionRefusal::Closed));
    drop(lease);
    admission.drained().await;
}
