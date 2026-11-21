use nessa_ports::{
    ApplyError, Change, CloseAction, Completion, CompletionError, ForwardError, Identity, Listener,
    LoopbackEndpoint, Ports, ReleaseReason, SnapshotError,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn id(value: &str) -> Identity {
    Identity::new(value).unwrap()
}
fn listener(name: &str, port: u16) -> Listener {
    Listener::new(id(name), IpAddr::V4(Ipv4Addr::LOCALHOST), port).unwrap()
}
fn address(value: &str) -> SocketAddr {
    value.parse().unwrap()
}
fn setup(capacity: usize) -> (Ports, Identity, Listener) {
    let workspace = id("workspace");
    let item = listener("listener-a", 3000);
    let mut ports = Ports::new(workspace.clone(), 1, capacity);
    assert_eq!(
        ports.apply(&workspace, 1, 1, [item.clone()]),
        Ok((Change::Changed, vec![]))
    );
    (ports, workspace, item)
}

#[test]
fn discovery_never_reserves_and_request_requires_exact_evidence() {
    let (mut ports, workspace, item) = setup(2);
    assert_eq!(ports.occupied(), 0);
    assert_eq!(
        ports.apply(&workspace, 1, 2, [item.clone()]),
        Ok((Change::Unchanged, vec![]))
    );
    assert_eq!(ports.occupied(), 0);
    for result in [
        ports.request(&workspace, 1, 1, &item),
        ports.request(&workspace, 2, 2, &item),
        ports.request(&id("other"), 1, 2, &item),
        ports.request(&workspace, 1, 2, &listener("listener-a", 3001)),
        ports.request(&workspace, 1, 2, &listener("listener-b", 3000)),
    ] {
        assert_eq!(result, Err(ForwardError::StaleInventory));
    }
    assert_eq!(ports.occupied(), 0);
    assert!(ports.request(&workspace, 1, 2, &item).is_ok());
}

#[test]
fn capacity_accounts_for_opening_and_cleanup_until_confirmation() {
    let (mut ports, workspace, item) = setup(1);
    let lease = ports.request(&workspace, 1, 1, &item).unwrap();
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::Capacity)
    );
    assert_eq!(ports.close(&lease), Ok(CloseAction::AwaitOpening));
    assert_eq!(ports.close(&lease), Ok(CloseAction::AlreadyClosing));
    assert_eq!(ports.lost(&lease), Err(ForwardError::WrongState));
    assert_eq!(ports.released(&lease), Err(ForwardError::WrongState));
    assert_eq!(
        ports.opened(&lease, address("127.0.0.1:4123")),
        Ok(Completion::Release(ReleaseReason::Closed))
    );
    assert_eq!(
        ports.opened(&lease, address("127.0.0.1:4123")),
        Err(CompletionError::NotOpening)
    );
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::Capacity)
    );
    assert_eq!(ports.released(&lease), Ok(()));
    assert_eq!(ports.released(&lease), Err(ForwardError::UnknownLease));
    assert!(ports.request(&workspace, 1, 1, &item).is_ok());
}

#[test]
fn invalid_endpoints_are_released_and_loopback_is_validated() {
    assert!(LoopbackEndpoint::new(address("127.0.0.1:1234")).is_some());
    assert!(LoopbackEndpoint::new(address("[::1]:1234")).is_some());
    for value in ["0.0.0.0:1234", "192.0.2.1:1234", "[::]:1234", "127.0.0.1:0"] {
        assert_eq!(LoopbackEndpoint::new(address(value)), None);
        let (mut ports, workspace, item) = setup(1);
        let lease = ports.request(&workspace, 1, 1, &item).unwrap();
        assert_eq!(
            ports.opened(&lease, address(value)),
            Ok(Completion::Release(ReleaseReason::InvalidEndpoint))
        );
        assert_eq!(ports.occupied(), 1);
        assert_eq!(ports.released(&lease), Ok(()));
    }
}

#[test]
fn disappearance_invalidates_opening_and_active_and_reappearance_does_not_revive() {
    let (mut ports, workspace, item) = setup(2);
    let opening = ports.request(&workspace, 1, 1, &item).unwrap();
    let active = ports.request(&workspace, 1, 1, &item).unwrap();
    assert!(matches!(
        ports.opened(&active, address("[::1]:1234")),
        Ok(Completion::Active(_))
    ));
    assert_eq!(
        ports.apply(&workspace, 1, 2, []),
        Ok((
            Change::Changed,
            vec![
                (opening.clone(), CloseAction::AwaitOpening),
                (active.clone(), CloseAction::Release)
            ]
        ))
    );
    assert_eq!(
        ports.apply(&workspace, 1, 3, [item.clone()]),
        Ok((Change::Changed, vec![]))
    );
    assert_eq!(
        ports.opened(&opening, address("127.0.0.1:1234")),
        Ok(Completion::Release(ReleaseReason::Gone))
    );
    assert_eq!(
        ports.request(&workspace, 1, 3, &item),
        Err(ForwardError::Capacity)
    );
    assert_eq!(ports.released(&opening), Ok(()));
    assert_eq!(ports.lost(&active), Ok(()));
    assert_eq!(ports.occupied(), 0);
    assert!(ports.request(&workspace, 1, 3, &item).is_ok());
}

#[test]
fn replacement_and_workspace_close_keep_late_work_accounted_for() {
    let (mut ports, workspace, item) = setup(1);
    let lease = ports.request(&workspace, 1, 1, &item).unwrap();
    assert_eq!(
        ports.close_all(),
        vec![(lease.clone(), CloseAction::AwaitOpening)]
    );
    assert_eq!(ports.close_all(), vec![]);
    assert_eq!(
        ports.apply(&workspace, 1, 2, []),
        Err(ApplyError {
            error: SnapshotError::WorkspaceClosed,
            cleanup: vec![]
        })
    );
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::WorkspaceClosed)
    );
    assert!(ports.replace_generation(1).is_err());
    assert_eq!(ports.replace_generation(2), Ok(vec![]));
    assert_eq!(
        ports.apply(&workspace, 1, 3, []),
        Err(ApplyError {
            error: SnapshotError::WrongWorkspace,
            cleanup: vec![]
        })
    );
    ports.apply(&workspace, 2, 0, [item.clone()]).unwrap();
    assert_eq!(
        ports.request(&workspace, 2, 0, &item),
        Err(ForwardError::Capacity)
    );
    assert_eq!(
        ports.opened(&lease, address("127.0.0.1:1234")),
        Ok(Completion::Release(ReleaseReason::Closed))
    );
    ports.released(&lease).unwrap();
    let next = ports.request(&workspace, 2, 0, &item).unwrap();
    assert_ne!(next, lease);
    assert_eq!(
        ports.replace_generation(3),
        Ok(vec![(next.clone(), CloseAction::AwaitOpening)])
    );
    assert_eq!(
        ports.opened(&next, address("127.0.0.1:1234")),
        Ok(Completion::Release(ReleaseReason::Replaced))
    );
}

#[test]
fn failure_loss_and_invalid_snapshots_preserve_rules() {
    let (mut ports, workspace, item) = setup(1);
    let lease = ports.request(&workspace, 1, 1, &item).unwrap();
    assert_eq!(
        ports.apply(&workspace, 1, 2, [item.clone(), item.clone()]),
        Err(ApplyError {
            error: SnapshotError::DuplicateListener,
            cleanup: vec![(lease.clone(), CloseAction::AwaitOpening)]
        })
    );
    assert_eq!(ports.open_failed(&lease), Ok(()));
    assert_eq!(
        ports.opened(&lease, address("127.0.0.1:1234")),
        Err(CompletionError::UnknownLease)
    );
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::StaleInventory)
    );
    assert_eq!(
        ports.apply(&workspace, 1, 2, [item.clone()]),
        Err(ApplyError {
            error: SnapshotError::StaleRevision,
            cleanup: vec![]
        })
    );
    assert_eq!(
        ports.apply(&workspace, 1, 3, [item.clone()]),
        Ok((Change::Changed, vec![]))
    );
    let lease = ports.request(&workspace, 1, 3, &item).unwrap();
    ports.opened(&lease, address("127.0.0.1:1234")).unwrap();
    assert_eq!(ports.released(&lease), Err(ForwardError::WrongState));
    assert_eq!(ports.lost(&lease), Ok(()));
    assert_eq!(ports.occupied(), 0);
    assert_eq!(ports.close(&lease), Err(ForwardError::UnknownLease));
    assert_eq!(
        ports.apply(&workspace, 1, 4, [item]),
        Ok((Change::Unchanged, vec![]))
    );
    assert_eq!(ports.occupied(), 0);
}

#[test]
fn lease_identity_cannot_cross_workspaces() {
    let (mut first, workspace, item) = setup(1);
    let lease = first.request(&workspace, 1, 1, &item).unwrap();
    let other_workspace = id("other");
    let mut other = Ports::new(other_workspace.clone(), 1, 1);
    other.apply(&other_workspace, 1, 1, [item.clone()]).unwrap();
    let other_lease = other.request(&other_workspace, 1, 1, &item).unwrap();
    assert_ne!(lease, other_lease);
    assert_eq!(
        other.opened(&lease, address("127.0.0.1:1234")),
        Err(CompletionError::UnknownLease)
    );
    assert_eq!(other.close(&lease), Err(ForwardError::UnknownLease));
    assert_eq!(other.occupied(), 1);
}

#[test]
fn changed_listener_and_failed_open_invalidate_without_leaking_slots() {
    let (mut ports, workspace, item) = setup(2);
    let opening = ports.request(&workspace, 1, 1, &item).unwrap();
    let active = ports.request(&workspace, 1, 1, &item).unwrap();
    ports.opened(&active, address("127.0.0.1:1234")).unwrap();
    assert_eq!(
        ports.apply(&workspace, 1, 2, [listener("listener-a", 3001)]),
        Ok((
            Change::Changed,
            vec![
                (opening.clone(), CloseAction::AwaitOpening),
                (active.clone(), CloseAction::Release)
            ]
        ))
    );
    assert_eq!(ports.open_failed(&opening), Ok(()));
    assert_eq!(ports.occupied(), 1);
    assert_eq!(ports.released(&active), Ok(()));
    assert_eq!(ports.occupied(), 0);
}

#[test]
fn explicit_close_and_replacement_release_active_resources() {
    let (mut ports, workspace, item) = setup(2);
    let closed = ports.request(&workspace, 1, 1, &item).unwrap();
    let replaced = ports.request(&workspace, 1, 1, &item).unwrap();
    ports.opened(&closed, address("127.0.0.1:1234")).unwrap();
    ports.opened(&replaced, address("127.0.0.1:4321")).unwrap();
    assert_eq!(ports.close(&closed), Ok(CloseAction::Release));
    assert_eq!(ports.close(&closed), Ok(CloseAction::AlreadyClosing));
    assert_eq!(
        ports.replace_generation(2),
        Ok(vec![(replaced.clone(), CloseAction::Release)])
    );
    assert_eq!(ports.occupied(), 2);
    ports.released(&closed).unwrap();
    ports.released(&replaced).unwrap();
    assert_eq!(ports.occupied(), 0);
}

#[test]
fn zero_capacity_refuses_even_valid_requests() {
    let (mut ports, workspace, item) = setup(0);
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::Capacity)
    );
    assert_eq!(ports.occupied(), 0);
}

#[test]
fn conflicting_snapshot_retires_active_lease_until_newer_evidence() {
    let (mut ports, workspace, item) = setup(1);
    let lease = ports.request(&workspace, 1, 1, &item).unwrap();
    ports.opened(&lease, address("127.0.0.1:1234")).unwrap();
    assert_eq!(
        ports.apply(&workspace, 1, 1, []),
        Err(ApplyError {
            error: SnapshotError::ConflictingRevision,
            cleanup: vec![(lease.clone(), CloseAction::Release)]
        })
    );
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::StaleInventory)
    );
    assert_eq!(
        ports.apply(&workspace, 1, 1, [item.clone()]),
        Err(ApplyError {
            error: SnapshotError::StaleRevision,
            cleanup: vec![]
        })
    );
    ports.released(&lease).unwrap();
    assert_eq!(
        ports.apply(&workspace, 1, 2, [item.clone()]),
        Ok((Change::Changed, vec![]))
    );
    assert_eq!(ports.inventory().revision(), Some(2));
    assert_eq!(
        ports.request(&workspace, 1, 1, &item),
        Err(ForwardError::StaleInventory)
    );
    assert!(ports.request(&workspace, 1, 2, &item).is_ok());
    assert_eq!(
        ports.apply(&workspace, 1, 3, [item.clone()]),
        Ok((Change::Unchanged, vec![]))
    );
    assert_eq!(ports.inventory().revision(), Some(3));
    assert_eq!(
        ports.request(&workspace, 1, 2, &item),
        Err(ForwardError::StaleInventory)
    );
}
