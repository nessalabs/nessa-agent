//! Each connection's window is held while the connection is open, the
//! newest revision wins however the publications arrive, and nobody reads
//! another person's windows.
use super::*;
use crate::conversation::domain::{
    PaneContent, WindowArrangement, WindowColumn, WindowPane, WindowPanel, WindowShowing,
};

fn window(sidebar: bool) -> WorkspaceWindow {
    WorkspaceWindow::new(
        WindowArrangement::Columns,
        sidebar,
        true,
        WindowShowing::Panes,
        WindowPanel::new(false, vec![], None).unwrap(),
        vec![WindowColumn::new(
            100,
            vec![WindowPane::new(100, true, PaneContent::Draft).unwrap()],
        )
        .unwrap()],
    )
    .unwrap()
}

fn owner(name: &str) -> (OrganizationId, PrincipalId) {
    (
        OrganizationId::new("org").unwrap(),
        PrincipalId::new(name).unwrap(),
    )
}

fn sidebars(windows: &WorkspaceWindows, name: &str) -> Vec<(u64, bool)> {
    let (org, principal) = owner(name);
    windows
        .of(&org, &principal)
        .iter()
        .map(|published| (published.revision, published.window.sidebar()))
        .collect()
}

const A: &str = "0b3b8f0e-6f43-4a51-9d55-0e7e4f7f8a11";
const B: &str = "1c4c9f1f-7054-4b62-8e66-1f8f5f8f9b22";

#[test]
fn a_window_is_held_from_its_publication_until_its_connection_closes() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    let first = windows.open(org.clone(), principal.clone());
    // Connected, nothing said yet: no window to read.
    assert!(sidebars(&windows, "owner").is_empty());
    assert_eq!(
        first.publish(A, 1, Some(window(true))),
        WindowPublished {
            revision: 1,
            applied: true
        }
    );
    let second = windows.open(org, principal);
    second.publish(B, 1, Some(window(false)));
    // The most recently published first.
    assert_eq!(sidebars(&windows, "owner"), [(1, false), (1, true)]);
    // A publication with no window withdraws it, and the place stays.
    second.publish(B, 2, None);
    assert_eq!(sidebars(&windows, "owner"), [(1, true)]);
    second.publish(B, 3, Some(window(true)));
    drop(first);
    assert_eq!(sidebars(&windows, "owner"), [(3, true)]);
    drop(second);
    assert!(sidebars(&windows, "owner").is_empty());
    assert!(
        windows.lock().is_empty(),
        "a closed owner leaves nothing behind"
    );
}

#[test]
fn the_newest_revision_wins_whichever_arrives_last() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    let slot = windows.open(org, principal);
    slot.publish(A, 5, Some(window(true)));
    assert_eq!(
        slot.publish(A, 4, Some(window(false))),
        WindowPublished {
            revision: 5,
            applied: false
        }
    );
    assert_eq!(sidebars(&windows, "owner"), [(5, true)]);
    // The same revision again is the same word, and applies.
    assert!(slot.publish(A, 5, Some(window(false))).applied);
    assert_eq!(sidebars(&windows, "owner"), [(5, false)]);
}

#[test]
fn a_window_that_reconnects_is_one_window_not_two() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    // The connection a window left behind, not yet seen to close.
    let stale = windows.open(org.clone(), principal.clone());
    stale.publish(A, 4, Some(window(true)));
    let fresh = windows.open(org, principal);
    fresh.publish(A, 5, Some(window(false)));
    assert_eq!(sidebars(&windows, "owner"), [(5, false)]);
    // A late answer on the old connection says less than the window's newest.
    assert_eq!(
        stale.publish(A, 4, Some(window(true))),
        WindowPublished {
            revision: 5,
            applied: false
        }
    );
    assert_eq!(sidebars(&windows, "owner"), [(5, false)]);
    // Withdrawn on its new connection, it is withdrawn, whatever the old one held.
    fresh.publish(A, 6, None);
    assert!(sidebars(&windows, "owner").is_empty());
    fresh.publish(A, 7, Some(window(false)));
    // The window closes on its new connection: the old one says nothing for it.
    drop(fresh);
    assert!(sidebars(&windows, "owner").is_empty());
    drop(stale);
}

#[test]
fn nobody_reads_another_persons_windows() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    let slot = windows.open(org, principal);
    slot.publish(A, 1, Some(window(true)));
    assert!(sidebars(&windows, "other").is_empty());
    let (_, owner) = owner("owner");
    assert!(windows
        .of(&OrganizationId::new("elsewhere").unwrap(), &owner)
        .is_empty());
}
