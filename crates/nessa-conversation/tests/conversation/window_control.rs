//! A change sent to a window is recorded as made only once the window says
//! so, as refused only when it is refused, and as unconfirmed when the window
//! went quiet or away after it was sent: it may still have made it.
use super::*;
use crate::application::{
    ConversationFuture, WindowAnswer, WindowControlAudit, WindowControlAuditRecord,
    WindowControlError, WindowControlOutcome, WindowControlVia, WindowTarget,
};
use crate::domain::{
    PaneContent, WindowAction, WindowArrangement, WindowColumn, WindowPane, WindowPanel,
    WindowShowing, WindowSide, WorkspaceWindow,
};

const WINDOW: &str = "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6";

#[derive(Default)]
struct Recorded(Mutex<Vec<WindowControlOutcome>>);

impl WindowControlAudit for Recorded {
    fn record(&self, record: WindowControlAuditRecord) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().push(record.outcome);
        Box::pin(async { Ok(()) })
    }
}

fn shown(sidebar: bool) -> WorkspaceWindow {
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

fn fold() -> WindowAction {
    WindowAction::Show {
        side: WindowSide::Sidebar,
        open: false,
    }
}

#[tokio::test(start_paused = true)]
async fn a_change_the_window_does_not_confirm_is_unconfirmed_not_refused() {
    let directory = tempfile::tempdir().unwrap();
    let (service, _, _) = fixture(
        &directory.path().join("data"),
        Arc::new(ProviderFactory::default()),
    );
    let audit = Arc::new(Recorded::default());
    service.bind_window_control_audit(audit.clone());
    let person = caller("window");
    let (slot, mut commands) = service
        .workspace_windows()
        .open(person.organization_id.clone(), person.principal_id.clone());
    slot.publish(WINDOW, 1, Some(shown(true)));
    let control = || {
        service.control_window(
            &person,
            WindowControlVia::Session,
            WindowTarget::Named(WINDOW),
            fold(),
        )
    };

    // Answered once its publication is held: made.
    let (done, ()) = tokio::join!(control(), async {
        let command = commands.recv().await.unwrap();
        slot.publish(WINDOW, 2, Some(shown(false)));
        assert!(slot.answer(WINDOW, command.id, WindowAnswer::Applied { revision: 2 }));
    });
    assert!(done.unwrap().changed);
    // Refused by the window: refused.
    let (done, ()) = tokio::join!(control(), async {
        let command = commands.recv().await.unwrap();
        let code = "no_room".to_owned();
        assert!(slot.answer(WINDOW, command.id, WindowAnswer::Refused { code }));
    });
    assert!(matches!(done, Err(WindowControlError::Refused(code)) if code == "no_room"));
    // Sent, and never answered: the window may yet carry it out.
    let (done, _sent) = tokio::join!(control(), commands.recv());
    assert_eq!(done.unwrap_err().code(), "window_not_answering");
    // Sent, and the connection closed before an answer.
    let (done, ()) = tokio::join!(control(), async {
        commands.recv().await.unwrap();
        drop(slot);
    });
    assert_eq!(done.unwrap_err().code(), "window_unavailable");
    // Nothing to send to: refused, never sent.
    assert_eq!(control().await.unwrap_err().code(), "window_unavailable");

    let outcomes = audit.0.lock().unwrap().clone();
    let requested = WindowControlOutcome::Requested;
    assert_eq!(
        outcomes,
        vec![
            requested.clone(),
            WindowControlOutcome::Applied { revision: 2 },
            requested.clone(),
            WindowControlOutcome::Refused {
                code: "no_room".into()
            },
            requested.clone(),
            WindowControlOutcome::Unconfirmed {
                code: "window_not_answering".into()
            },
            requested,
            WindowControlOutcome::Unconfirmed {
                code: "window_unavailable".into()
            },
            WindowControlOutcome::Refused {
                code: "window_unavailable".into()
            },
        ]
    );
}
