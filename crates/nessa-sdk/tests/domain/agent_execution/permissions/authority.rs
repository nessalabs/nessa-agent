//! Weak owner reads preserve typed faults without retaining decisions.
use super::*;

#[test]
fn permission_authority_reports_poison_and_drop_without_retaining_owner() {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let authority = PermissionAuthority::new(
        ExecutionSessionId::new("session").unwrap(),
        ExecutionId::new("execution").unwrap(),
        &pending,
    );
    let id = PermissionId::new("permission").unwrap();
    let _ = std::panic::catch_unwind(|| {
        let _guard = pending.lock().unwrap();
        panic!("poison pending owner");
    });
    assert_eq!(
        authority.pending(&id),
        Err(PermissionAuthorityError::Poisoned)
    );
    drop(pending);
    assert_eq!(
        authority.pending(&id),
        Err(PermissionAuthorityError::Unavailable)
    );
}
