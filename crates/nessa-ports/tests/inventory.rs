use nessa_ports::{
    Change, Identity, InvalidIdentity, InvalidPort, Inventory, Listener, SnapshotError,
    StaleGeneration,
};
use std::net::{IpAddr, Ipv4Addr};

fn id(value: &str) -> Identity {
    Identity::new(value).unwrap()
}
fn listener(name: &str, port: u16) -> Listener {
    Listener::new(id(name), IpAddr::V4(Ipv4Addr::LOCALHOST), port).unwrap()
}

#[test]
fn values_reject_empty_identity_and_zero_port() {
    assert_eq!(Identity::new(" \t"), Err(InvalidIdentity));
    assert_eq!(
        Listener::new(id("a"), IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        Err(InvalidPort)
    );
}

#[test]
fn complete_snapshots_are_revisioned_and_order_independent() {
    let workspace = id("workspace");
    let mut inventory = Inventory::new(workspace.clone(), 2);
    assert_eq!(
        inventory.apply(&workspace, 2, 1, [listener("a", 3000), listener("b", 3000)]),
        Ok(Change::Changed)
    );
    assert_eq!(
        inventory.apply(&workspace, 2, 2, [listener("b", 3000), listener("a", 3000)]),
        Ok(Change::Unchanged)
    );
    assert_eq!(
        inventory.apply(&workspace, 2, 2, [listener("a", 3000), listener("b", 3000)]),
        Ok(Change::Unchanged)
    );
    assert_eq!(inventory.revision(), Some(2));
    assert_eq!(
        inventory.apply(&workspace, 2, 2, [listener("a", 3000)]),
        Err(SnapshotError::ConflictingRevision)
    );
    assert_eq!(
        inventory.apply(&workspace, 2, 1, []),
        Err(SnapshotError::StaleRevision)
    );
    assert_eq!(inventory.listeners().count(), 2);
    assert_eq!(inventory.apply(&workspace, 2, 3, []), Ok(Change::Changed));
    assert_eq!(inventory.listeners().count(), 0);
}

#[test]
fn ambiguous_and_old_workspace_updates_leave_state_intact() {
    let workspace = id("workspace");
    let mut inventory = Inventory::new(workspace.clone(), 4);
    assert_eq!(
        inventory.apply(&workspace, 4, 8, [listener("a", 3000)]),
        Ok(Change::Changed)
    );
    assert_eq!(
        inventory.apply(&workspace, 4, 9, [listener("a", 3000), listener("a", 3001)]),
        Err(SnapshotError::DuplicateListener)
    );
    assert_eq!(
        inventory.apply(&workspace, 3, 9, []),
        Err(SnapshotError::WrongWorkspace)
    );
    assert_eq!(
        inventory.apply(&id("other"), 4, 9, []),
        Err(SnapshotError::WrongWorkspace)
    );
    assert_eq!(inventory.revision(), Some(8));
    assert_eq!(
        inventory.listeners().collect::<Vec<_>>(),
        vec![&listener("a", 3000)]
    );
}

#[test]
fn replacement_retires_the_old_generation() {
    let workspace = id("workspace");
    let mut inventory = Inventory::new(workspace.clone(), 4);
    inventory
        .apply(&workspace, 4, 8, [listener("a", 3000)])
        .unwrap();
    assert_eq!(inventory.replace_generation(4), Err(StaleGeneration));
    assert_eq!(inventory.revision(), Some(8));
    assert_eq!(inventory.replace_generation(5), Ok(()));
    assert_eq!(inventory.revision(), None);
    assert_eq!(inventory.listeners().count(), 0);
    assert_eq!(
        inventory.apply(&workspace, 4, 9, []),
        Err(SnapshotError::WrongWorkspace)
    );
    assert_eq!(inventory.apply(&workspace, 5, 0, []), Ok(Change::Changed));
}
