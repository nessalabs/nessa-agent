//! Linux: the desktop notification service, `org.freedesktop.Notifications`,
//! on the session bus.
//!
//! The service names each notice with a number it chooses, so this keeps
//! which number is which review, and which conversation, for as long as the
//! notice is up. `CloseNotification` takes one back; `ActionInvoked` with the
//! `default` action is a click. A notice the service reports closed (the
//! person dismissed it) is forgotten, so a later clear never closes a number
//! the service has since handed to someone else.
//!
//! The service keeps no names across a restart of this app, so a notice a
//! previous run left up cannot be found again: `withdraw_all` closes only this
//! run's. Each notice is posted to stay until it is closed, which this run does
//! as soon as the review settles.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use zbus::blocking::{connection::Builder as ConnectionBuilder, Connection, Proxy};
use zbus::zvariant::Value;

use crate::review_notices::application::{NoticeCenter, NoticeOpened};
use crate::review_notices::domain::{NoticeAccess, NoticeId, ReviewNotice};

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const DEFAULT_ACTION: &str = "default";
/// The service answers from the same session in milliseconds.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Stays up until this app closes it.
const NEVER_EXPIRES: i32 = 0;

type Connect = dyn Fn() -> zbus::Result<Connection> + Send + Sync;

#[derive(Default)]
struct Shown {
    by_review: HashMap<NoticeId, u32>,
    conversation_by_number: HashMap<u32, String>,
}

impl Shown {
    fn forget_number(&mut self, number: u32) {
        if self.conversation_by_number.remove(&number).is_some() {
            self.by_review.retain(|_, shown| *shown != number);
        }
    }
}

/// The session's notification service, reached on first use.
pub(crate) struct FreedesktopNotices {
    connect: Box<Connect>,
    opened: Arc<dyn NoticeOpened>,
    connection: Mutex<Option<Connection>>,
    /// Whether the service reads the body as markup, asked once per connection.
    markup: Mutex<Option<bool>>,
    shown: Arc<Mutex<Shown>>,
}

impl FreedesktopNotices {
    /// The notices on this login's session bus.
    pub(crate) fn session(opened: Arc<dyn NoticeOpened>) -> Self {
        Self::new(
            Box::new(|| {
                ConnectionBuilder::session()?
                    .method_timeout(CALL_TIMEOUT)
                    .build()
            }),
            opened,
        )
    }

    fn new(connect: Box<Connect>, opened: Arc<dyn NoticeOpened>) -> Self {
        Self {
            connect,
            opened,
            connection: Mutex::new(None),
            markup: Mutex::new(None),
            shown: Arc::default(),
        }
    }

    /// The live connection, connecting and starting the click listener when
    /// there is none.
    fn connection(&self) -> zbus::Result<Connection> {
        let mut held = lock(&self.connection);
        if let Some(connection) = held.as_ref() {
            return Ok(connection.clone());
        }
        let connection = (self.connect)()?;
        // Subscribed before the first post, so no click is missed.
        let signals = proxy(&connection)?.receive_all_signals()?;
        let shown = Arc::clone(&self.shown);
        let opened = Arc::clone(&self.opened);
        std::thread::Builder::new()
            .name("nessa-review-notices".to_owned())
            .spawn(move || {
                for signal in signals {
                    let header = signal.header();
                    let Some(member) = header.member() else {
                        continue;
                    };
                    match member.as_str() {
                        "ActionInvoked" => {
                            let Ok((number, action)) = signal.body().deserialize::<(u32, String)>()
                            else {
                                continue;
                            };
                            if action != DEFAULT_ACTION {
                                continue;
                            }
                            let conversation =
                                lock(&shown).conversation_by_number.get(&number).cloned();
                            if let Some(conversation) = conversation {
                                opened.opened(&conversation);
                            }
                        }
                        "NotificationClosed" => {
                            if let Ok((number, _reason)) = signal.body().deserialize::<(u32, u32)>()
                            {
                                lock(&shown).forget_number(number);
                            }
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|error| zbus::Error::Failure(error.to_string()))?;
        *held = Some(connection.clone());
        Ok(connection)
    }

    /// Forget a connection that failed, so the next call connects again.
    fn drop_connection(&self) {
        lock(&self.connection).take();
        lock(&self.markup).take();
    }

    /// Whether the service reads the body as markup. Escaping for one that
    /// does not would show the person `&amp;`.
    fn body_markup(&self) -> zbus::Result<bool> {
        if let Some(markup) = *lock(&self.markup) {
            return Ok(markup);
        }
        let capabilities =
            self.call(|proxy| proxy.call::<_, _, Vec<String>>("GetCapabilities", &()))?;
        let markup = capabilities
            .iter()
            .any(|capability| capability == "body-markup");
        *lock(&self.markup) = Some(markup);
        Ok(markup)
    }

    fn call<R>(&self, call: impl FnOnce(&Proxy<'_>) -> zbus::Result<R>) -> zbus::Result<R> {
        let connection = self.connection()?;
        let outcome = proxy(&connection).and_then(|proxy| call(&proxy));
        if let Err(zbus::Error::InputOutput(_)) = &outcome {
            self.drop_connection();
        }
        outcome
    }
}

fn proxy(connection: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(connection, DESTINATION, PATH, INTERFACE)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Neither value holds an invariant a panic could break halfway.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A service that reads the body as markup gets the person's own text escaped.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl NoticeCenter for FreedesktopNotices {
    /// Linux has no per-app permission: a running service shows notices.
    fn access(&self) -> NoticeAccess {
        match self.call(|proxy| {
            proxy.call::<_, _, (String, String, String, String)>("GetServerInformation", &())
        }) {
            Ok(_) => NoticeAccess::Allowed,
            Err(_) => NoticeAccess::Unsupported,
        }
    }

    fn ask(&self) -> NoticeAccess {
        self.access()
    }

    fn post(&self, notice: &ReviewNotice) -> Result<(), String> {
        // Replacing the number this review already has keeps one notice per
        // review, the way the macOS identifier does.
        let replaces = lock(&self.shown)
            .by_review
            .get(&notice.id)
            .copied()
            .unwrap_or(0);
        let hints: HashMap<&str, Value<'_>> = HashMap::new();
        let body = if self.body_markup().map_err(|error| error.to_string())? {
            escaped(&notice.body)
        } else {
            notice.body.clone()
        };
        let number = self
            .call(|proxy| {
                proxy.call::<_, _, u32>(
                    "Notify",
                    &(
                        "Nessa",
                        replaces,
                        "",
                        notice.title.as_str(),
                        body,
                        vec![DEFAULT_ACTION, "Open"],
                        hints,
                        NEVER_EXPIRES,
                    ),
                )
            })
            .map_err(|error| error.to_string())?;
        let mut shown = lock(&self.shown);
        if let Some(previous) = shown.by_review.insert(notice.id.clone(), number) {
            shown.conversation_by_number.remove(&previous);
        }
        shown
            .conversation_by_number
            .insert(number, notice.id.conversation().to_owned());
        Ok(())
    }

    fn withdraw(&self, id: &NoticeId) -> Result<(), String> {
        let Some(number) = lock(&self.shown).by_review.get(id).copied() else {
            // Never shown, or the person already dismissed it.
            return Ok(());
        };
        self.call(|proxy| proxy.call::<_, _, ()>("CloseNotification", &(number,)))
            .map_err(|error| error.to_string())?;
        // Forgotten only once it is closed, so a failed close is tried again.
        lock(&self.shown).forget_number(number);
        Ok(())
    }

    fn withdraw_all(&self) {
        let numbers: Vec<u32> = {
            let mut shown = lock(&self.shown);
            let numbers = shown.by_review.values().copied().collect();
            *shown = Shown::default();
            numbers
        };
        for number in numbers {
            if let Err(error) =
                self.call(|proxy| proxy.call::<_, _, ()>("CloseNotification", &(number,)))
            {
                crate::host_log::warn(format!("a review notice was not closed: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Against a fake notification service on a private bus that each test
    //! starts and stops itself.

    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;

    use zbus::blocking::Connection;
    use zbus::interface;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Notify {
            replaces: u32,
            summary: String,
            body: String,
            expires: i32,
        },
        Close(u32),
    }

    struct FakeService {
        next: u32,
        markup: bool,
        calls: mpsc::Sender<Call>,
    }

    #[interface(name = "org.freedesktop.Notifications")]
    impl FakeService {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &mut self,
            _app_name: String,
            replaces_id: u32,
            _app_icon: String,
            summary: String,
            body: String,
            _actions: Vec<String>,
            _hints: HashMap<String, zbus::zvariant::OwnedValue>,
            expire_timeout: i32,
        ) -> u32 {
            let _ = self.calls.send(Call::Notify {
                replaces: replaces_id,
                summary,
                body,
                expires: expire_timeout,
            });
            if replaces_id != 0 {
                return replaces_id;
            }
            self.next += 1;
            self.next
        }

        fn close_notification(&self, id: u32) {
            let _ = self.calls.send(Call::Close(id));
        }

        fn get_capabilities(&self) -> Vec<String> {
            let mut capabilities = vec!["actions".to_owned()];
            if self.markup {
                capabilities.push("body-markup".to_owned());
            }
            capabilities
        }

        fn get_server_information(&self) -> (String, String, String, String) {
            ("fake".into(), "nessa".into(), "1".into(), "1.2".into())
        }
    }

    struct Bus {
        daemon: Child,
        address: String,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    fn private_bus() -> Bus {
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("the Linux test host has dbus-daemon");
        let mut line = String::new();
        let stdout = daemon.stdout.take().unwrap();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line).unwrap();
        Bus {
            daemon,
            address: line.trim().to_owned(),
        }
    }

    struct Opened(mpsc::Sender<String>);

    impl NoticeOpened for Opened {
        fn opened(&self, conversation: &str) {
            let _ = self.0.send(conversation.to_owned());
        }
    }

    struct Fixture {
        _bus: Bus,
        service: Connection,
        notices: FreedesktopNotices,
        calls: mpsc::Receiver<Call>,
        opened: mpsc::Receiver<String>,
    }

    fn fixture() -> Fixture {
        fixture_reading_markup(true)
    }

    fn fixture_reading_markup(markup: bool) -> Fixture {
        let bus = private_bus();
        let (calls_send, calls) = mpsc::channel();
        let service = ConnectionBuilder::address(bus.address.as_str())
            .unwrap()
            .name(DESTINATION)
            .unwrap()
            .serve_at(
                PATH,
                FakeService {
                    next: 0,
                    markup,
                    calls: calls_send,
                },
            )
            .unwrap()
            .build()
            .unwrap();
        let address = bus.address.clone();
        let (opened_send, opened) = mpsc::channel();
        let notices = FreedesktopNotices::new(
            Box::new(move || {
                ConnectionBuilder::address(address.as_str())?
                    .method_timeout(CALL_TIMEOUT)
                    .build()
            }),
            Arc::new(Opened(opened_send)),
        );
        Fixture {
            _bus: bus,
            service,
            notices,
            calls,
            opened,
        }
    }

    fn notice(conversation: &str, permission: &str) -> ReviewNotice {
        ReviewNotice::new(
            NoticeId::new(conversation, "e", permission).unwrap(),
            "Bash needs a decision",
            "Fix <the> build & ship",
        )
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_running_service_allows_notices() {
        let fixture = fixture();
        assert_eq!(fixture.notices.access(), NoticeAccess::Allowed);
        assert_eq!(fixture.notices.ask(), NoticeAccess::Allowed);
    }

    #[test]
    fn no_service_is_unsupported() {
        let bus = private_bus();
        let address = bus.address.clone();
        let (send, _) = mpsc::channel();
        let notices = FreedesktopNotices::new(
            Box::new(move || {
                ConnectionBuilder::address(address.as_str())?
                    .method_timeout(CALL_TIMEOUT)
                    .build()
            }),
            Arc::new(Opened(send)),
        );
        assert_eq!(notices.access(), NoticeAccess::Unsupported);
        assert!(notices.post(&notice("c", "p")).is_err());
    }

    #[test]
    fn a_notice_stays_until_it_is_closed() {
        let fixture = fixture();
        fixture.notices.post(&notice("c", "p")).unwrap();
        assert_eq!(
            fixture.calls.recv_timeout(WAIT).unwrap(),
            Call::Notify {
                replaces: 0,
                summary: "Bash needs a decision".into(),
                body: "Fix &lt;the&gt; build &amp; ship".into(),
                expires: NEVER_EXPIRES,
            }
        );
        fixture.notices.withdraw(&notice("c", "p").id).unwrap();
        assert_eq!(fixture.calls.recv_timeout(WAIT).unwrap(), Call::Close(1));
        // Already closed: nothing left to close.
        fixture.notices.withdraw(&notice("c", "p").id).unwrap();
        fixture.notices.post(&notice("c", "q")).unwrap();
        assert!(matches!(
            fixture.calls.recv_timeout(WAIT).unwrap(),
            Call::Notify { replaces: 0, .. }
        ));
    }

    #[test]
    fn a_plain_text_service_gets_the_body_as_it_is() {
        let fixture = fixture_reading_markup(false);
        fixture.notices.post(&notice("c", "p")).unwrap();
        assert!(matches!(
            fixture.calls.recv_timeout(WAIT).unwrap(),
            Call::Notify { body, .. } if body == "Fix <the> build & ship"
        ));
    }

    #[test]
    fn the_same_review_replaces_its_own_notice() {
        let fixture = fixture();
        fixture.notices.post(&notice("c", "p")).unwrap();
        fixture.notices.post(&notice("c", "p")).unwrap();
        let _ = fixture.calls.recv_timeout(WAIT).unwrap();
        assert!(matches!(
            fixture.calls.recv_timeout(WAIT).unwrap(),
            Call::Notify { replaces: 1, .. }
        ));
    }

    #[test]
    fn a_click_opens_the_conversation() {
        let fixture = fixture();
        fixture.notices.post(&notice("conv-a", "p")).unwrap();
        fixture
            .service
            .emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "ActionInvoked",
                &(1u32, "default"),
            )
            .unwrap();
        assert_eq!(fixture.opened.recv_timeout(WAIT).unwrap(), "conv-a");
    }

    #[test]
    fn a_dismissed_notice_is_never_closed_again() {
        let fixture = fixture();
        fixture.notices.post(&notice("c", "p")).unwrap();
        let _ = fixture.calls.recv_timeout(WAIT).unwrap();
        fixture
            .service
            .emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "NotificationClosed",
                &(1u32, 2u32),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + WAIT;
        while lock(&fixture.notices.shown)
            .by_review
            .contains_key(&notice("c", "p").id)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the close was not seen"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        fixture.notices.withdraw_all();
        fixture.notices.post(&notice("c", "q")).unwrap();
        assert!(matches!(
            fixture.calls.recv_timeout(WAIT).unwrap(),
            Call::Notify { .. }
        ));
    }

    #[test]
    fn withdraw_all_closes_this_runs_notices() {
        let fixture = fixture();
        fixture.notices.post(&notice("c", "p")).unwrap();
        fixture.notices.post(&notice("c", "q")).unwrap();
        let _ = fixture.calls.recv_timeout(WAIT).unwrap();
        let _ = fixture.calls.recv_timeout(WAIT).unwrap();
        fixture.notices.withdraw_all();
        let mut closed = vec![
            fixture.calls.recv_timeout(WAIT).unwrap(),
            fixture.calls.recv_timeout(WAIT).unwrap(),
        ];
        closed.sort_by_key(|call| format!("{call:?}"));
        assert_eq!(closed, [Call::Close(1), Call::Close(2)]);
    }
}
