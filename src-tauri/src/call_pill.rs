//! The call pill: a small window that floats over every app while a voice
//! call is open and the desktop window is not in front, with the call's
//! switches on it — microphone, speaker, End.
//!
//! The call itself lives in the desktop window's page. This module only
//! carries two things across: what the call looks like now, from the desktop
//! page to the pill, and what the person pressed, from the pill back. The
//! host keeps the latest picture so a pill page that loads late can ask for
//! it (`call_pill_read`) rather than wait for the next change.
//!
//! Who may call what is decided here, by the calling window's label: only the
//! desktop window says what its call looks like, and only the pill sends
//! presses. Whether the pill is on screen is decided here too, from the
//! desktop window's own focus, so a page that cannot tell it left the front
//! (the keyboard was inside a frame) still gets its pill.
//!
//! The windows and the window server are outside the process: `CallPill`
//! reaches them only through `PillScreen`, which composition supplies
//! (`HostDependencies::call_pill`) and the tests here stand in for.
//!
//! The window is declared hidden in `tauri.conf.json`, is never destroyed
//! while the app runs, and is placed once — top centre of the main screen —
//! then left wherever the person drags it.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewWindow};

use crate::composition::HostDependencies;
use crate::desktop_window::{self, DESKTOP_WINDOW};
use crate::host;

/// The window's label, shared with `tauri.conf.json` and `capabilities/call.json`.
pub const CALL_WINDOW: &str = "call";

/// The most of a chat's title the pill is handed; the pill shows far less.
const MAX_TITLE_CHARS: usize = 120;

/// How far below the top of the screen's work area the pill first appears, in points.
const TOP_GAP: f64 = 10.0;

/// Where an open call stands, in the voice controller's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallPhase {
    Connecting,
    Listening,
    Working,
    Speaking,
}

/// The call as the pill draws it. The JSON is the contract with
/// `src/host/call-pill.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct CallPicture {
    phase: CallPhase,
    /// The chat the call talks to, by its title.
    title: String,
    microphone_on: bool,
    speaker_on: bool,
}

/// A picture's fields as they are named on the wire, and no others.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PictureFields {
    phase: CallPhase,
    title: String,
    microphone_on: bool,
    speaker_on: bool,
}

impl TryFrom<serde_json::Value> for CallPicture {
    type Error = String;

    /// Only an object with exactly a picture's fields is one. A derived
    /// reader alone would also take the same values as a bare list.
    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        if !value.is_object() {
            return Err("a call picture is an object".into());
        }
        let fields: PictureFields =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        Ok(Self {
            phase: fields.phase,
            title: fields.title,
            microphone_on: fields.microphone_on,
            speaker_on: fields.speaker_on,
        })
    }
}

impl CallPicture {
    /// The picture with its title cut to what the pill is handed.
    fn bounded(mut self) -> Self {
        if self.title.chars().count() > MAX_TITLE_CHARS {
            self.title = self.title.chars().take(MAX_TITLE_CHARS).collect();
        }
        self
    }
}

/// What the person pressed on the pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallPress {
    End,
    Mute,
    Unmute,
    SpeakerOff,
    SpeakerOn,
    /// The chat's name: bring the desktop window forward.
    Open,
}

/// The window server and the two pages, as the pill needs them. They are
/// outside the process, so the pill reaches them through this seam
/// (`HostDependencies::call_pill` is built with the real one) and a test
/// stands in for it.
pub trait PillScreen: Send + Sync {
    /// Whether the desktop window is the one in front. A window server that
    /// will not say counts as not in front: the pill then shows, which costs
    /// a pill beside a window that has the same switches.
    fn desktop_in_front(&self) -> bool;
    /// Hands the pill's page what to draw; none clears it.
    fn draw(&self, picture: Option<&CallPicture>) -> Result<(), String>;
    /// Whether the pill's window is on screen; none when there is no such window.
    fn on_screen(&self) -> Option<bool>;
    fn hide(&self) -> Result<(), String>;
    /// Puts the pill on screen without handing it the keyboard.
    fn show(&self) -> Result<(), String>;
    /// The main screen's work area and the pill's size; `Ok(None)` when
    /// there is no screen to measure.
    fn room(&self) -> Result<Option<Room>, String>;
    fn move_to(&self, at: Place) -> Result<(), String>;
    /// Hands a press to the desktop window's page, which owns the call.
    fn press(&self, press: CallPress) -> Result<(), String>;
    /// Brings the desktop window forward.
    fn open_desktop(&self);
}

/// A point on the screens, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub x: i32,
    pub y: i32,
}

/// A width and height, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

/// What placing the pill for the first time is measured against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// The work area's top left corner.
    pub area_at: Place,
    pub area: Extent,
    pub pill: Extent,
    /// Physical pixels per point on that screen.
    pub scale: f64,
}

/// The call pill: the latest picture, whether the pill has been placed yet,
/// and the screen it is shown on.
pub struct CallPill {
    screen: Arc<dyn PillScreen>,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    picture: Option<CallPicture>,
    placed: bool,
}

impl CallPill {
    pub fn new(screen: Arc<dyn PillScreen>) -> Self {
        Self {
            screen,
            held: Mutex::default(),
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        // A poisoned lock still holds a whole value: both fields are written
        // in one assignment each.
        self.held.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// What the desktop window's call looks like now; none when no call is
    /// open. Whether the pill then shows is decided here, from the window.
    pub fn set(&self, call: Option<CallPicture>) {
        self.held().picture = call.map(CallPicture::bounded);
        self.settle(self.screen.desktop_in_front());
    }

    /// The desktop window came to the front or left it.
    pub fn desktop_front_changed(&self, in_front: bool) {
        self.settle(in_front);
    }

    /// The desktop page is loading again: whatever call it held is gone with it.
    pub fn desktop_page_reloading(&self) {
        self.held().picture = None;
        self.settle(true);
    }

    /// The picture the pill should be drawing now; none when no call is shown.
    pub fn read(&self) -> Option<CallPicture> {
        self.drawn(self.screen.desktop_in_front())
    }

    /// A press on the pill, handed to the desktop window, which owns the call.
    pub fn press(&self, press: CallPress) -> Result<(), String> {
        if press == CallPress::Open {
            self.screen.open_desktop();
        }
        self.screen
            .press(press)
            .map_err(|error| format!("could not reach the call: {error}"))
    }

    /// Brings the pill's window in line with the held picture: drawn and on
    /// screen, or cleared and hidden. Failures cost the pill, never the
    /// call: each is logged, and the next change tries again.
    fn settle(&self, desktop_in_front: bool) {
        let picture = self.drawn(desktop_in_front);
        if let Err(error) = self.screen.draw(picture.as_ref()) {
            crate::host_log::warn(format!("could not update the call pill: {error}"));
        }
        let Some(visible) = self.screen.on_screen() else {
            return;
        };
        if picture.is_none() {
            if visible {
                if let Err(error) = self.screen.hide() {
                    crate::host_log::warn(format!("could not hide the call pill: {error}"));
                }
            }
            return;
        }
        if visible {
            return;
        }
        if !std::mem::replace(&mut self.held().placed, true) {
            self.place();
        }
        if let Err(error) = self.screen.show() {
            crate::host_log::warn(format!("could not show the call pill: {error}"));
        }
    }

    /// Puts the pill at the top centre of the main screen. Losing the
    /// placement costs a pill in the wrong corner, not the pill, so failures
    /// are logged and the pill is shown where it is.
    fn place(&self) {
        let room = match self.screen.room() {
            Ok(Some(room)) => room,
            Ok(None) => return,
            Err(error) => {
                crate::host_log::warn(format!("could not measure for the call pill: {error}"));
                return;
            }
        };
        if let Err(error) = self.screen.move_to(first_place(room)) {
            crate::host_log::warn(format!("could not place the call pill: {error}"));
        }
    }

    /// What the pill draws now: the held picture while it shows, else none.
    fn drawn(&self, desktop_in_front: bool) -> Option<CallPicture> {
        let held = self.held();
        if shows(held.picture.is_some(), desktop_in_front) {
            held.picture.clone()
        } else {
            None
        }
    }
}

/// Top centre of a work area: the pill's first place.
fn first_place(room: Room) -> Place {
    let space = i64::from(room.area.width) - i64::from(room.pill.width);
    let x = i64::from(room.area_at.x) + (space / 2).max(0);
    let gap = if room.scale.is_finite() && room.scale > 0.0 {
        (TOP_GAP * room.scale).round() as i64
    } else {
        TOP_GAP as i64
    };
    let y = i64::from(room.area_at.y) + gap;
    Place {
        x: i32::try_from(x).unwrap_or(room.area_at.x),
        y: i32::try_from(y).unwrap_or(room.area_at.y),
    }
}

/// Whether the pill is on screen: a call is open, and the desktop window is
/// not the one in front. In front, the composer already has the switches.
fn shows(call_open: bool, desktop_in_front: bool) -> bool {
    call_open && !desktop_in_front
}

/// The windows Tauri holds, and the window server behind them.
struct NativePillScreen(AppHandle);

impl NativePillScreen {
    fn pill(&self) -> Result<WebviewWindow, String> {
        self.0
            .get_webview_window(CALL_WINDOW)
            .ok_or_else(|| "there is no call pill window".to_owned())
    }
}

impl PillScreen for NativePillScreen {
    fn desktop_in_front(&self) -> bool {
        self.0
            .get_webview_window(DESKTOP_WINDOW)
            .is_some_and(|desktop| desktop.is_focused().unwrap_or(false))
    }

    fn draw(&self, picture: Option<&CallPicture>) -> Result<(), String> {
        self.0
            .emit_to(CALL_WINDOW, host::CALL_PILL_PICTURE, picture)
            .map_err(|error| error.to_string())
    }

    fn on_screen(&self) -> Option<bool> {
        let pill = self.0.get_webview_window(CALL_WINDOW)?;
        Some(pill.is_visible().unwrap_or(false))
    }

    fn hide(&self) -> Result<(), String> {
        self.pill()?.hide().map_err(|error| error.to_string())
    }

    fn show(&self) -> Result<(), String> {
        crate::platform::current().show_quietly(&self.pill()?)
    }

    fn room(&self) -> Result<Option<Room>, String> {
        let pill = self.pill()?;
        let Some(monitor) = pill.primary_monitor().map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        let size = pill.outer_size().map_err(|error| error.to_string())?;
        let area = monitor.work_area();
        Ok(Some(Room {
            area_at: Place {
                x: area.position.x,
                y: area.position.y,
            },
            area: Extent {
                width: area.size.width,
                height: area.size.height,
            },
            pill: Extent {
                width: size.width,
                height: size.height,
            },
            scale: monitor.scale_factor(),
        }))
    }

    fn move_to(&self, at: Place) -> Result<(), String> {
        self.pill()?
            .set_position(PhysicalPosition::new(at.x, at.y))
            .map_err(|error| error.to_string())
    }

    fn press(&self, press: CallPress) -> Result<(), String> {
        self.0
            .emit_to(DESKTOP_WINDOW, host::CALL_PILL_PRESS, press)
            .map_err(|error| error.to_string())
    }

    fn open_desktop(&self) {
        desktop_window::open(&self.0);
    }
}

/// The call pill on this app's own windows.
pub fn call_pill(app: &AppHandle) -> Arc<CallPill> {
    Arc::new(CallPill::new(Arc::new(NativePillScreen(app.clone()))))
}

fn only(window: &WebviewWindow, label: &str) -> Result<(), String> {
    if window.label() == label {
        Ok(())
    } else {
        Err(format!("only the {label} window may call this"))
    }
}

/// What the desktop window's call looks like now; none when no call is open.
/// The desktop page says this at every change, whichever window is in front.
#[tauri::command]
pub fn call_pill_set(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    call: Option<CallPicture>,
) -> Result<(), String> {
    only(&window, DESKTOP_WINDOW)?;
    deps.call_pill.set(call);
    Ok(())
}

/// The picture the pill should be drawing now; none when no call is shown.
#[tauri::command]
pub fn call_pill_read(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<Option<CallPicture>, String> {
    only(&window, CALL_WINDOW)?;
    Ok(deps.call_pill.read())
}

/// A press on the pill, handed to the desktop window, which owns the call.
#[tauri::command]
pub fn call_pill_press(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    press: CallPress,
) -> Result<(), String> {
    only(&window, CALL_WINDOW)?;
    deps.call_pill.press(press)
}

/// Moves the pill by a drag that began on it. The pill has no title bar, so
/// its page asks for the drag; nothing else about the window is the page's.
/// The drag is the calling window's own, begun under the pointer that asked.
#[tauri::command]
pub fn call_pill_drag(window: WebviewWindow) -> Result<(), String> {
    only(&window, CALL_WINDOW)?;
    window
        .start_dragging()
        .map_err(|error| format!("could not move the call pill: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen that answers as told and writes down what it was asked.
    #[derive(Default)]
    struct Screen {
        said: Mutex<Said>,
    }

    struct Said {
        desktop_in_front: bool,
        /// None: there is no pill window.
        on_screen: Option<bool>,
        room: Result<Option<Room>, String>,
        refuses: Vec<&'static str>,
        did: Vec<String>,
    }

    impl Default for Said {
        fn default() -> Self {
            Self {
                desktop_in_front: true,
                on_screen: Some(false),
                room: Ok(Some(Room {
                    area_at: Place { x: 0, y: 50 },
                    area: Extent {
                        width: 3024,
                        height: 1900,
                    },
                    pill: Extent {
                        width: 480,
                        height: 104,
                    },
                    scale: 2.0,
                })),
                refuses: Vec::new(),
                did: Vec::new(),
            }
        }
    }

    impl Screen {
        fn said(&self) -> std::sync::MutexGuard<'_, Said> {
            self.said.lock().expect("the test screen")
        }

        fn does(&self, what: &'static str, wrote: String) -> Result<(), String> {
            let mut said = self.said();
            said.did.push(wrote);
            if said.refuses.contains(&what) {
                Err(format!("{what} refused"))
            } else {
                Ok(())
            }
        }

        /// What was asked since the last look.
        fn did(&self) -> Vec<String> {
            std::mem::take(&mut self.said().did)
        }
    }

    impl PillScreen for Screen {
        fn desktop_in_front(&self) -> bool {
            self.said().desktop_in_front
        }
        fn draw(&self, picture: Option<&CallPicture>) -> Result<(), String> {
            let wrote = match picture {
                Some(picture) => format!("draw {}", picture.title),
                None => "draw nothing".to_owned(),
            };
            self.does("draw", wrote)
        }
        fn on_screen(&self) -> Option<bool> {
            self.said().on_screen
        }
        fn hide(&self) -> Result<(), String> {
            let done = self.does("hide", "hide".to_owned());
            if done.is_ok() {
                self.said().on_screen = Some(false);
            }
            done
        }
        fn show(&self) -> Result<(), String> {
            let done = self.does("show", "show".to_owned());
            if done.is_ok() {
                self.said().on_screen = Some(true);
            }
            done
        }
        fn room(&self) -> Result<Option<Room>, String> {
            self.said().room.clone()
        }
        fn move_to(&self, at: Place) -> Result<(), String> {
            self.does("move", format!("move {},{}", at.x, at.y))
        }
        fn press(&self, press: CallPress) -> Result<(), String> {
            self.does("press", format!("press {press:?}"))
        }
        fn open_desktop(&self) {
            self.said().did.push("open desktop".to_owned());
        }
    }

    fn scene() -> (Arc<Screen>, CallPill) {
        let screen = Arc::new(Screen::default());
        (screen.clone(), CallPill::new(screen))
    }

    fn call(title: &str) -> Option<CallPicture> {
        Some(CallPicture {
            phase: CallPhase::Listening,
            title: title.into(),
            microphone_on: true,
            speaker_on: true,
        })
    }

    fn leave_front(screen: &Screen, pill: &CallPill) {
        screen.said().desktop_in_front = false;
        pill.desktop_front_changed(false);
    }

    fn come_front(screen: &Screen, pill: &CallPill) {
        screen.said().desktop_in_front = true;
        pill.desktop_front_changed(true);
    }

    #[test]
    fn the_pill_shows_only_for_an_open_call_behind_other_windows() {
        assert!(shows(true, false));
        assert!(!shows(true, true));
        assert!(!shows(false, false));
        assert!(!shows(false, true));
    }

    #[test]
    fn a_call_in_the_front_window_keeps_the_pill_away_and_leaving_shows_it_placed_once() {
        let (screen, pill) = scene();
        pill.set(call("Fix the build"));
        assert_eq!(screen.did(), ["draw nothing"]);
        assert_eq!(pill.read(), None);

        leave_front(&screen, &pill);
        assert_eq!(screen.did(), ["draw Fix the build", "move 1272,70", "show"]);
        assert_eq!(pill.read(), call("Fix the build"));

        // On screen already: a change is drawn, and nothing is shown or moved.
        pill.set(call("Ship the pill"));
        assert_eq!(screen.did(), ["draw Ship the pill"]);

        come_front(&screen, &pill);
        assert_eq!(screen.did(), ["draw nothing", "hide"]);

        // The second time it stays wherever it was left.
        leave_front(&screen, &pill);
        assert_eq!(screen.did(), ["draw Ship the pill", "show"]);
    }

    #[test]
    fn no_call_shows_no_pill_and_an_ended_call_hides_it() {
        let (screen, pill) = scene();
        leave_front(&screen, &pill);
        assert_eq!(screen.did(), ["draw nothing"]);
        pill.set(call("Fix the build"));
        screen.did();
        pill.set(None);
        assert_eq!(screen.did(), ["draw nothing", "hide"]);
        assert_eq!(pill.read(), None);
    }

    #[test]
    fn a_desktop_page_loading_again_puts_the_pill_away_with_its_call() {
        let (screen, pill) = scene();
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        screen.did();
        pill.desktop_page_reloading();
        assert_eq!(screen.did(), ["draw nothing", "hide"]);
        assert_eq!(pill.read(), None);
        // The window is still behind: with no call there is nothing to bring back.
        pill.desktop_front_changed(false);
        assert_eq!(screen.did(), ["draw nothing"]);
    }

    #[test]
    fn a_show_the_window_server_refused_is_tried_again_at_the_next_change() {
        let (screen, pill) = scene();
        screen.said().refuses = vec!["show"];
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        assert_eq!(
            screen.did(),
            ["draw nothing", "draw Fix the build", "move 1272,70", "show"]
        );
        screen.said().refuses.clear();
        pill.set(call("Fix the build"));
        assert_eq!(screen.did(), ["draw Fix the build", "show"]);
        assert_eq!(screen.said().on_screen, Some(true));
    }

    #[test]
    fn a_hide_the_window_server_refused_is_tried_again_at_the_next_change() {
        let (screen, pill) = scene();
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        screen.did();
        screen.said().refuses = vec!["hide"];
        pill.set(None);
        assert_eq!(screen.did(), ["draw nothing", "hide"]);
        screen.said().refuses.clear();
        come_front(&screen, &pill);
        assert_eq!(screen.did(), ["draw nothing", "hide"]);
        assert_eq!(screen.said().on_screen, Some(false));
    }

    #[test]
    fn a_page_that_cannot_be_reached_or_a_place_that_cannot_be_had_still_shows_the_pill() {
        let (screen, pill) = scene();
        screen.said().refuses = vec!["draw", "move"];
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        assert_eq!(screen.did().last().map(String::as_str), Some("show"));
        // The late page asks for what it missed.
        assert_eq!(pill.read(), call("Fix the build"));

        let (screen, pill) = scene();
        screen.said().room = Err("no screens".into());
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        assert_eq!(screen.did(), ["draw nothing", "draw Fix the build", "show"]);

        let (screen, pill) = scene();
        screen.said().room = Ok(None);
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        assert_eq!(screen.did(), ["draw nothing", "draw Fix the build", "show"]);
    }

    #[test]
    fn with_no_pill_window_the_picture_is_still_kept_and_nothing_is_shown() {
        let (screen, pill) = scene();
        screen.said().on_screen = None;
        leave_front(&screen, &pill);
        pill.set(call("Fix the build"));
        assert_eq!(screen.did(), ["draw nothing", "draw Fix the build"]);
        assert_eq!(pill.read(), call("Fix the build"));
    }

    #[test]
    fn a_press_reaches_the_call_and_the_chat_s_name_opens_the_desktop_window_first() {
        let (screen, pill) = scene();
        pill.press(CallPress::Mute).expect("a press");
        pill.press(CallPress::Open).expect("a press");
        assert_eq!(screen.did(), ["press Mute", "open desktop", "press Open"]);
        screen.said().refuses = vec!["press"];
        assert_eq!(
            pill.press(CallPress::End),
            Err("could not reach the call: press refused".to_owned())
        );
    }

    #[test]
    fn the_pill_first_stands_at_the_top_centre_of_the_work_area() {
        let at = first_place(Room {
            area_at: Place { x: 0, y: 50 },
            area: Extent {
                width: 3024,
                height: 1900,
            },
            pill: Extent {
                width: 480,
                height: 104,
            },
            scale: 2.0,
        });
        assert_eq!(at, Place { x: 1272, y: 70 });
    }

    #[test]
    fn a_screen_to_the_left_keeps_its_own_origin() {
        let at = first_place(Room {
            area_at: Place { x: -1920, y: 0 },
            area: Extent {
                width: 1920,
                height: 1080,
            },
            pill: Extent {
                width: 240,
                height: 52,
            },
            scale: 1.0,
        });
        assert_eq!(at, Place { x: -1080, y: 10 });
    }

    #[test]
    fn a_work_area_narrower_than_the_pill_starts_it_at_the_edge() {
        let at = first_place(Room {
            area_at: Place { x: 0, y: 0 },
            area: Extent {
                width: 100,
                height: 100,
            },
            pill: Extent {
                width: 240,
                height: 52,
            },
            scale: f64::NAN,
        });
        assert_eq!(at, Place { x: 0, y: 10 });
    }

    #[test]
    fn a_picture_is_read_in_the_page_s_words_and_nothing_else() {
        let picture: CallPicture = serde_json::from_str(
            r#"{"phase":"listening","title":"Fix the build","microphoneOn":true,"speakerOn":false}"#,
        )
        .expect("the page's picture");
        assert_eq!(picture.phase, CallPhase::Listening);
        assert!(picture.microphone_on && !picture.speaker_on);
        for refused in [
            r#"{"phase":"idle","title":"","microphoneOn":true,"speakerOn":true}"#,
            r#"{"phase":"listening","title":"","microphoneOn":true,"speakerOn":true,"extra":1}"#,
            r#"{"phase":"listening","title":"","microphoneOn":"yes","speakerOn":true}"#,
            r#"["listening","",true,true]"#,
        ] {
            assert!(
                serde_json::from_str::<CallPicture>(refused).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_long_title_is_cut_on_a_character() {
        let picture = CallPicture {
            phase: CallPhase::Working,
            title: "é".repeat(MAX_TITLE_CHARS + 30),
            microphone_on: true,
            speaker_on: true,
        }
        .bounded();
        assert_eq!(picture.title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn presses_are_named_as_the_page_names_them() {
        for (press, name) in [
            (CallPress::End, "\"end\""),
            (CallPress::Mute, "\"mute\""),
            (CallPress::Unmute, "\"unmute\""),
            (CallPress::SpeakerOff, "\"speaker-off\""),
            (CallPress::SpeakerOn, "\"speaker-on\""),
            (CallPress::Open, "\"open\""),
        ] {
            assert_eq!(serde_json::to_string(&press).expect("a press"), name);
            assert_eq!(serde_json::from_str::<CallPress>(name).expect(name), press);
        }
        assert!(serde_json::from_str::<CallPress>("\"quit\"").is_err());
    }
}
