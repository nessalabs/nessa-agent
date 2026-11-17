//! Each connection's window is held while the connection is open, the
//! newest revision wins however the publications arrive, and nobody reads
//! another person's windows.
use super::*;
use crate::domain::{
    PaneContent, WindowAction, WindowArrangement, WindowColumn, WindowPane, WindowPanel,
    WindowShowing,
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
    let first = windows.open(org.clone(), principal.clone()).0;
    // Connected, nothing said yet: no window to read.
    assert!(sidebars(&windows, "owner").is_empty());
    assert_eq!(
        first.publish(A, 1, Some(window(true))),
        WindowPublished {
            revision: 1,
            applied: true
        }
    );
    let second = windows.open(org, principal).0;
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
    let slot = windows.open(org, principal).0;
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
    let stale = windows.open(org.clone(), principal.clone()).0;
    stale.publish(A, 4, Some(window(true)));
    let fresh = windows.open(org, principal).0;
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
    let slot = windows.open(org, principal).0;
    slot.publish(A, 1, Some(window(true)));
    assert!(sidebars(&windows, "other").is_empty());
    let (_, owner) = owner("owner");
    assert!(windows
        .of(&OrganizationId::new("elsewhere").unwrap(), &owner)
        .is_empty());
}

fn sidebar(open: bool) -> WindowAction {
    WindowAction::Show {
        side: crate::domain::WindowSide::Sidebar,
        open,
    }
}

#[tokio::test]
async fn a_command_goes_to_the_connection_holding_the_window_and_its_answer_comes_back() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    let (stale, mut stale_commands) = windows.open(org.clone(), principal.clone());
    let (fresh, mut fresh_commands) = windows.open(org.clone(), principal.clone());
    stale.publish(A, 1, Some(window(true)));
    // The window came back on another connection: that one holds it now.
    fresh.publish(A, 2, Some(window(true)));
    let answered = windows
        .command(&org, &principal, A, sidebar(false))
        .unwrap();
    let command = fresh_commands.try_recv().unwrap();
    assert_eq!(command.window_id, A);
    assert_eq!(command.action, sidebar(false));
    assert!(stale_commands.try_recv().is_err());
    // Only the connection that was asked settles it, and only for its window.
    assert!(!stale.answer(A, command.id, WindowAnswer::Applied { revision: 3 }));
    assert!(!fresh.answer(B, command.id, WindowAnswer::Applied { revision: 3 }));
    assert!(fresh.answer(A, command.id, WindowAnswer::Applied { revision: 3 }));
    assert_eq!(
        answered.await.unwrap(),
        WindowAnswer::Applied { revision: 3 }
    );
    // Answered once.
    assert!(!fresh.answer(A, command.id, WindowAnswer::Applied { revision: 3 }));
}

#[test]
fn a_command_for_a_window_nobody_holds_is_refused() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    assert_eq!(
        windows
            .command(&org, &principal, A, sidebar(true))
            .unwrap_err(),
        CommandRefusal::Gone
    );
    let (slot, _commands) = windows.open(org.clone(), principal.clone());
    slot.publish(A, 1, Some(window(true)));
    // Another person's window is not theirs to change.
    let (_, stranger) = owner("stranger");
    assert_eq!(
        windows
            .command(&org, &stranger, A, sidebar(true))
            .unwrap_err(),
        CommandRefusal::Gone
    );
    // Withdrawn, it shows nothing to change.
    slot.publish(A, 2, None);
    assert_eq!(
        windows
            .command(&org, &principal, A, sidebar(true))
            .unwrap_err(),
        CommandRefusal::Gone
    );
}

#[tokio::test]
async fn a_window_that_stops_answering_is_busy_and_its_closing_ends_every_wait() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    let (slot, commands) = windows.open(org.clone(), principal.clone());
    slot.publish(A, 1, Some(window(true)));
    let waits: Vec<_> = (0..MAX_WINDOW_COMMANDS)
        .map(|_| windows.command(&org, &principal, A, sidebar(true)).unwrap())
        .collect();
    assert_eq!(
        windows
            .command(&org, &principal, A, sidebar(true))
            .unwrap_err(),
        CommandRefusal::Busy
    );
    drop(commands);
    drop(slot);
    for wait in waits {
        assert!(wait.await.is_err());
    }
}

#[tokio::test]
async fn a_wait_for_a_revision_ends_once_it_is_published() {
    let windows = WorkspaceWindows::new();
    let (org, principal) = owner("owner");
    let (slot, _commands) = windows.open(org.clone(), principal.clone());
    slot.publish(A, 1, Some(window(true)));
    let waiting = {
        let windows = windows.clone();
        let (org, principal) = (org.clone(), principal.clone());
        tokio::spawn(async move { windows.published(&org, &principal, A, 2).await })
    };
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    slot.publish(A, 2, Some(window(false)));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap()
    );
    // Already held: no wait at all.
    assert!(windows.published(&org, &principal, A, 1).await);
    // Its connection closes while a wait is on: the wait hears it is gone.
    let waiting = {
        let windows = windows.clone();
        let (org, principal) = (org.clone(), principal.clone());
        tokio::spawn(async move { windows.published(&org, &principal, A, 3).await })
    };
    tokio::task::yield_now().await;
    drop(slot);
    assert!(
        !tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap()
    );
}
