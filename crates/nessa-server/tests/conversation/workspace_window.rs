//! A window is what it published, checked: its panes' places read in the
//! words a person uses, and a window no screen could draw is refused.
use super::*;

fn widget(plugin: &str, id: &str) -> WidgetRef {
    WidgetRef::new(plugin.into(), id.into()).unwrap()
}

fn conversation(n: u8) -> PaneContent {
    PaneContent::Conversation(
        ConversationId::new(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap(),
    )
}

fn pane(focused: bool, content: PaneContent) -> WindowPane {
    WindowPane::new(50, focused, content).unwrap()
}

fn shut() -> WindowPanel {
    WindowPanel::new(false, vec![], None).unwrap()
}

fn window(columns: Vec<WindowColumn>) -> Result<WorkspaceWindow, WindowRefusal> {
    WorkspaceWindow::new(
        WindowArrangement::Columns,
        true,
        true,
        WindowShowing::Panes,
        shut(),
        columns,
    )
}

fn positions(window: &WorkspaceWindow) -> Vec<(usize, String)> {
    window
        .places()
        .into_iter()
        .map(|place| (place.number, place.position))
        .collect()
}

#[test]
fn a_pane_is_placed_in_the_words_a_person_uses() {
    let one = window(vec![WindowColumn::new(
        100,
        vec![pane(true, conversation(1))],
    )
    .unwrap()])
    .unwrap();
    assert_eq!(positions(&one), [(1, "full".to_owned())]);

    let two = window(vec![
        WindowColumn::new(50, vec![pane(true, conversation(1))]).unwrap(),
        WindowColumn::new(50, vec![pane(false, conversation(2))]).unwrap(),
    ])
    .unwrap();
    assert_eq!(
        positions(&two),
        [(1, "left".to_owned()), (2, "right".to_owned())]
    );

    // Reading order: down each column, then across.
    let stacked = window(vec![
        WindowColumn::new(34, vec![pane(false, conversation(1))]).unwrap(),
        WindowColumn::new(33, vec![pane(false, PaneContent::Draft)]).unwrap(),
        WindowColumn::new(
            33,
            vec![
                pane(true, conversation(2)),
                pane(false, conversation(3)),
                pane(false, conversation(4)),
            ],
        )
        .unwrap(),
    ])
    .unwrap();
    assert_eq!(
        positions(&stacked),
        [
            (1, "left".to_owned()),
            (2, "middle".to_owned()),
            (3, "top right".to_owned()),
            (4, "middle right".to_owned()),
            (5, "bottom right".to_owned()),
        ]
    );
    let places = stacked.places();
    assert_eq!((places[3].column, places[3].row), (3, 2));
    assert_eq!(places[0].column_width_percent, 34);
}

#[test]
fn a_window_no_screen_could_draw_is_refused() {
    assert_eq!(window(vec![]), Err(WindowRefusal::Shape));
    assert_eq!(WindowColumn::new(50, vec![]), Err(WindowRefusal::Shape));
    let full = || {
        (0..=MAX_COLUMN_PANES)
            .map(|_| pane(false, PaneContent::Draft))
            .collect()
    };
    assert_eq!(WindowColumn::new(50, full()), Err(WindowRefusal::Shape));
    let column = || WindowColumn::new(10, vec![pane(false, PaneContent::Draft)]).unwrap();
    assert_eq!(
        window((0..=MAX_WINDOW_COLUMNS).map(|_| column()).collect()),
        Err(WindowRefusal::Shape)
    );
    assert_eq!(
        WindowColumn::new(101, vec![pane(false, PaneContent::Draft)]),
        Err(WindowRefusal::Share)
    );
    assert_eq!(
        WindowPane::new(u64::from(u8::MAX) + 1, false, PaneContent::Draft),
        Err(WindowRefusal::Share)
    );
    assert_eq!(
        window(vec![WindowColumn::new(
            100,
            vec![pane(true, conversation(1)), pane(true, conversation(2))]
        )
        .unwrap()]),
        Err(WindowRefusal::Focus)
    );
    assert_eq!(
        WidgetRef::new(String::new(), "id".into()),
        Err(WindowRefusal::Widget)
    );
    assert_eq!(
        WidgetRef::new("plugin".into(), "x".repeat(MAX_WIDGET_ID_BYTES + 1)),
        Err(WindowRefusal::Widget)
    );
    assert!(WidgetRef::new("plugin".into(), "x".repeat(MAX_WIDGET_ID_BYTES)).is_ok());
}

#[test]
fn a_panels_front_is_one_of_its_tabs_and_sixteen_is_the_last_it_holds() {
    let subagents = widget("subagents", "chat");
    let app = widget("mcp:huddle", "call-1");
    let open = WindowPanel::new(
        true,
        vec![subagents.clone(), app.clone()],
        Some(app.clone()),
    );
    let open = open.unwrap();
    assert!(open.open());
    assert_eq!(open.tabs(), &[subagents.clone(), app.clone()]);
    assert_eq!(open.front(), Some(&app));

    assert_eq!(
        WindowPanel::new(true, vec![subagents.clone()], None),
        Err(WindowRefusal::Panel)
    );
    assert_eq!(
        WindowPanel::new(false, vec![], Some(subagents.clone())),
        Err(WindowRefusal::Panel)
    );
    assert_eq!(
        WindowPanel::new(
            true,
            vec![subagents.clone()],
            Some(widget("subagents", "other"))
        ),
        Err(WindowRefusal::Panel)
    );
    assert_eq!(
        WindowPanel::new(
            true,
            vec![subagents.clone(), subagents.clone()],
            Some(subagents)
        ),
        Err(WindowRefusal::Panel)
    );
    let sixteen: Vec<_> = (0..MAX_PANEL_TABS)
        .map(|index| widget("subagents", &index.to_string()))
        .collect();
    assert!(WindowPanel::new(true, sixteen.clone(), Some(sixteen[0].clone())).is_ok());
    let mut seventeen = sixteen;
    seventeen.push(widget("subagents", "more"));
    assert_eq!(
        WindowPanel::new(true, seventeen, None),
        Err(WindowRefusal::Panel)
    );
}
