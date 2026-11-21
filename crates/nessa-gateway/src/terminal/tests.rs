//! The hub's rules for several views of one terminal, against a fake engine
//! that records what it is asked. The same rules as the TLA+ model in
//! `docs/state/tla/services/desktop/workspace/share-a-terminal-between-views.tla`.
use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorded(
    Mutex<Vec<String>>,
    Mutex<Vec<tokio::sync::mpsc::Sender<TerminalSignal>>>,
);

struct FakeRunning(Arc<Recorded>);

impl RunningTerminal for FakeRunning {
    fn send(&self, command: EngineCommand) {
        let line = match command {
            EngineCommand::Attach { key, out } => {
                self.0 .1.lock().unwrap().push(out);
                format!("attach {}", key.view)
            }
            EngineCommand::Detach { key } => format!("detach {}", key.view),
            EngineCommand::Input(bytes) => format!("input {}", String::from_utf8_lossy(&bytes)),
            EngineCommand::Resize(size) => format!("resize {}x{}", size.cols, size.rows),
            EngineCommand::History { key, start, count } => {
                format!("history {} {start} {count}", key.view)
            }
            EngineCommand::Close => "close".to_owned(),
        };
        self.0 .0.lock().unwrap().push(line);
    }
}

/// Run while a terminal starts, with its id and where it reports.
type DuringStart = Box<dyn FnOnce(&str, Arc<dyn EngineEvents>) + Send>;

#[derive(Default)]
struct FakeEngine {
    recorded: Arc<Recorded>,
    events: Mutex<Option<Arc<dyn EngineEvents>>>,
    starts: std::sync::atomic::AtomicUsize,
    during_start: Mutex<Option<DuringStart>>,
}

impl TerminalEngine for FakeEngine {
    fn start(
        &self,
        id: &str,
        _spec: &StartSpec,
        events: Arc<dyn EngineEvents>,
    ) -> Result<Box<dyn RunningTerminal>, TerminalError> {
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.events.lock().unwrap() = Some(events.clone());
        let during = self.during_start.lock().unwrap().take();
        if let Some(during) = during {
            during(id, events);
        }
        Ok(Box::new(FakeRunning(self.recorded.clone())))
    }
}

struct Fixed;
impl TerminalClock for Fixed {
    fn now_ms(&self) -> u64 {
        1_000
    }
}

fn owner(name: &str) -> TerminalOwner {
    TerminalOwner {
        organization: "org".into(),
        principal: name.into(),
    }
}

fn size(cols: u16, rows: u16) -> GridSize {
    GridSize { cols, rows }
}

fn request(owner: &TerminalOwner, id: &str) -> OpenRequest {
    OpenRequest {
        owner: owner.clone(),
        request_id: id.into(),
        channel_id: None,
        conversation_id: None,
        folder: "/tmp".into(),
        kind: TerminalKind::Shell,
        size: size(80, 24),
    }
}

fn setup() -> (Arc<TerminalHub>, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::default());
    let hub = TerminalHub::new(Some(engine.clone()), Arc::new(Fixed));
    (hub, engine)
}

fn log(engine: &FakeEngine) -> Vec<String> {
    std::mem::take(&mut *engine.recorded.0.lock().unwrap())
}

#[test]
fn a_build_without_an_engine_refuses_every_open() {
    let hub = TerminalHub::new(None, Arc::new(Fixed));
    let (link, _out) = hub.connect(owner("p"));
    assert_eq!(
        link.open(request(&owner("p"), "r1"), "/bin/sh".into()),
        Err(TerminalError::Unavailable)
    );
}

#[test]
fn a_retried_open_answers_the_first_terminal() {
    let (hub, _engine) = setup();
    let (link, _out) = hub.connect(owner("p"));
    let first = link
        .open(request(&owner("p"), "r1"), "/bin/zsh".into())
        .unwrap();
    let again = link
        .open(request(&owner("p"), "r1"), "/bin/zsh".into())
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(first.title, "zsh");
    assert_eq!(hub.list(&owner("p")).len(), 1);
}

#[test]
fn the_focused_view_sets_the_size_and_an_unowned_terminal_is_taken_by_the_next_view() {
    let (hub, engine) = setup();
    let (link, _out) = hub.connect(owner("p"));
    let id = link
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap()
        .id;
    // Nobody owns a new terminal: the first view to open takes it.
    link.attach(&id, "a", size(120, 40)).unwrap();
    link.attach(&id, "b", size(60, 20)).unwrap();
    assert_eq!(log(&engine), ["attach a", "resize 120x40", "attach b"]);
    assert_eq!(hub.size_of(&owner("p"), &id).unwrap().1, size(120, 40));

    // Another view's box changing only rescales that view.
    link.fit(&id, "b", size(70, 20)).unwrap();
    assert!(log(&engine).is_empty());
    // The owner's box changing resizes.
    link.fit(&id, "a", size(100, 40)).unwrap();
    assert_eq!(log(&engine), ["resize 100x40"]);
    // Focus moves the size to the other view's fit.
    link.focus(&id, "b", size(70, 20)).unwrap();
    assert_eq!(log(&engine), ["resize 70x20"]);
    // Focus again at the same fit changes nothing.
    link.focus(&id, "b", size(70, 20)).unwrap();
    assert!(log(&engine).is_empty());
    // The owner goes: the size stays, and nobody owns it.
    link.detach(&id, "b").unwrap();
    assert_eq!(log(&engine), ["detach b"]);
    assert_eq!(hub.size_of(&owner("p"), &id), Some((None, size(70, 20))));
    // The next box change takes it, as a pane laid out again does.
    link.fit(&id, "a", size(90, 30)).unwrap();
    assert_eq!(log(&engine), ["resize 90x30"]);
    // A view that comes back while another owns it does not take it.
    link.attach(&id, "c", size(40, 10)).unwrap();
    assert_eq!(log(&engine), ["attach c"]);
    // A pane laid out again (its old view gone, a new one opened) takes the
    // size on open, though another view is still open.
    link.detach(&id, "a").unwrap();
    link.attach(&id, "d", size(50, 25)).unwrap();
    assert_eq!(log(&engine), ["detach a", "attach d", "resize 50x25"]);
    assert_eq!(hub.size_of(&owner("p"), &id).unwrap().1, size(50, 25));
}

#[test]
fn a_closed_connection_takes_its_views_and_its_ownership() {
    let (hub, engine) = setup();
    let (first, _a) = hub.connect(owner("p"));
    let (second, _b) = hub.connect(owner("p"));
    let id = first
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap()
        .id;
    first.attach(&id, "same", size(80, 24)).unwrap();
    second.attach(&id, "same", size(80, 24)).unwrap();
    second.focus(&id, "same", size(100, 30)).unwrap();
    log(&engine);
    drop(second);
    assert_eq!(log(&engine), ["detach same"]);
    assert_eq!(hub.size_of(&owner("p"), &id), Some((None, size(100, 30))));
}

#[test]
fn another_person_cannot_reach_a_terminal() {
    let (hub, engine) = setup();
    let (mine, _a) = hub.connect(owner("p"));
    let (theirs, _b) = hub.connect(owner("q"));
    let id = mine
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap()
        .id;
    log(&engine);
    assert_eq!(
        theirs.attach(&id, "v", size(80, 24)),
        Err(TerminalError::NotFound)
    );
    assert_eq!(
        theirs.input(&id, b"ls\r".to_vec()),
        Err(TerminalError::NotFound)
    );
    assert_eq!(theirs.close(&id), Err(TerminalError::NotFound));
    assert!(theirs.list().unwrap().is_empty());
    assert!(log(&engine).is_empty());
}

#[test]
fn an_ended_program_is_listed_and_refuses_input() {
    let (hub, engine) = setup();
    let (link, mut out) = hub.connect(owner("p"));
    link.list().unwrap();
    let id = link
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap()
        .id;
    assert_eq!(out.try_recv(), Some(TerminalSignal::Listed));
    let events = engine.events.lock().unwrap().clone().unwrap();
    events.exited(&id, Some(3));
    assert_eq!(out.try_recv(), Some(TerminalSignal::Listed));
    let entry = &hub.list(&owner("p"))[0];
    assert_eq!(
        entry.ended,
        Some(TerminalEnd {
            status: Some(3),
            at_ms: 1_000
        })
    );
    assert_eq!(link.input(&id, b"x".to_vec()), Err(TerminalError::NotFound));
    link.close(&id).unwrap();
    assert!(hub.list(&owner("p")).is_empty());
}

#[test]
fn a_run_is_titled_by_its_first_line() {
    assert_eq!(run_title("\n  npm run dev  \nnext"), "npm run dev");
    assert_eq!(run_title(&"x".repeat(90)).chars().count(), 81);
}

#[test]
fn a_grid_outside_what_the_runtime_takes_is_refused() {
    assert!(GridSize::new(1, 10).is_none());
    assert!(GridSize::new(80, 0).is_none());
    assert!(GridSize::new(1001, 10).is_none());
    assert_eq!(GridSize::new(80, 24), Some(size(80, 24)));
}

#[test]
fn a_terminal_takes_the_name_its_person_gives_it() {
    let (hub, _engine) = setup();
    let (link, _out) = hub.connect(owner("p"));
    let id = link
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap()
        .id;
    link.rename(&id, "build watcher".into()).unwrap();
    assert_eq!(hub.list(&owner("p"))[0].title, "build watcher");
    // Another person cannot name it.
    let (other, _o) = hub.connect(owner("q"));
    assert!(other.rename(&id, "mine".into()).is_err());
    assert_eq!(hub.list(&owner("p"))[0].title, "build watcher");
    // A name is trimmed, 1 to 80 characters, with no control characters.
    assert_eq!(super::named("  logs  ").as_deref(), Some("logs"));
    assert_eq!(super::named("   "), None);
    assert_eq!(super::named("a\u{7}b"), None);
    assert_eq!(super::named(&"x".repeat(81)), None);
    assert!(super::named(&"x".repeat(80)).is_some());
}

#[test]
fn a_retry_while_the_first_starts_answers_it_and_starts_nothing() {
    let (hub, engine) = setup();
    let (link, _out) = hub.connect(owner("p"));
    let retry = Arc::clone(&hub);
    let answered = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&answered);
    *engine.during_start.lock().unwrap() = Some(Box::new(move |_, _| {
        *seen.lock().unwrap() = Some(retry.open(request(&owner("p"), "r1"), "/bin/sh".into()));
    }));
    let first = link
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap();
    let retried = answered.lock().unwrap().take().unwrap().unwrap();
    assert_eq!(retried.id, first.id);
    assert_eq!(engine.starts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(hub.list(&owner("p")).len(), 1);
}

#[test]
fn a_terminal_still_starting_counts_toward_the_limit() {
    let (hub, engine) = setup();
    let (link, _out) = hub.connect(owner("p"));
    for n in 1..MAX_TERMINALS_PER_OWNER {
        link.open(request(&owner("p"), &format!("r{n}")), "/bin/sh".into())
            .unwrap();
    }
    let other = Arc::clone(&hub);
    let answered = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&answered);
    *engine.during_start.lock().unwrap() = Some(Box::new(move |_, _| {
        *seen.lock().unwrap() = Some(other.open(request(&owner("p"), "other"), "/bin/sh".into()));
    }));
    link.open(request(&owner("p"), "last"), "/bin/sh".into())
        .unwrap();
    assert_eq!(
        answered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .map(|entry| entry.id),
        Err(TerminalError::Limit)
    );
}

#[test]
fn an_exit_while_starting_is_kept() {
    let (hub, engine) = setup();
    let (link, _out) = hub.connect(owner("p"));
    *engine.during_start.lock().unwrap() = Some(Box::new(|id, events| {
        events.exited(id, Some(0));
    }));
    link.open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap();
    assert_eq!(
        hub.list(&owner("p"))[0].ended,
        Some(TerminalEnd {
            status: Some(0),
            at_ms: 1_000
        })
    );
}

#[test]
fn a_full_output_queue_never_loses_a_list_change() {
    let (hub, engine) = setup();
    let (link, mut out) = hub.connect(owner("p"));
    link.list().unwrap();
    let id = link
        .open(request(&owner("p"), "r1"), "/bin/sh".into())
        .unwrap()
        .id;
    assert_eq!(out.try_recv(), Some(TerminalSignal::Listed));
    link.attach(&id, "v1", size(80, 24)).unwrap();
    let sender = engine.recorded.1.lock().unwrap()[0].clone();
    while sender
        .try_send(TerminalSignal::Output {
            terminal_id: id.clone(),
            view_id: "v1".into(),
            message: String::new(),
        })
        .is_ok()
    {}
    link.rename(&id, "build".into()).unwrap();
    link.close(&id).unwrap();
    // Two changes while the queue was full: one word that the list changed.
    assert_eq!(out.try_recv(), Some(TerminalSignal::Listed));
    assert!(matches!(
        out.try_recv(),
        Some(TerminalSignal::Output { .. })
    ));
}
