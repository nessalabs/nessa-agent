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
//! The window is declared hidden in `tauri.conf.json`, is never destroyed
//! while the app runs, and is placed once — top centre of the main screen —
//! then left wherever the person drags it.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow};

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

/// The latest picture, and whether the pill has been placed yet.
#[derive(Default)]
pub struct CallPill {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    picture: Option<CallPicture>,
    placed: bool,
}

impl CallPill {
    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        // A poisoned lock still holds a whole value: both fields are written
        // in one assignment each.
        self.held.lock().unwrap_or_else(|error| error.into_inner())
    }
}

fn only(window: &WebviewWindow, label: &str) -> Result<(), String> {
    if window.label() == label {
        Ok(())
    } else {
        Err(format!("only the {label} window may call this"))
    }
}

/// Top centre of a work area, in physical pixels: the pill's first place.
fn first_place(
    area_at: PhysicalPosition<i32>,
    area: PhysicalSize<u32>,
    pill: PhysicalSize<u32>,
    scale: f64,
) -> PhysicalPosition<i32> {
    let room = i64::from(area.width) - i64::from(pill.width);
    let x = i64::from(area_at.x) + (room / 2).max(0);
    let gap = if scale.is_finite() && scale > 0.0 {
        (TOP_GAP * scale).round() as i64
    } else {
        TOP_GAP as i64
    };
    let y = i64::from(area_at.y) + gap;
    PhysicalPosition::new(
        i32::try_from(x).unwrap_or(area_at.x),
        i32::try_from(y).unwrap_or(area_at.y),
    )
}

/// Puts the pill at the top centre of the main screen. Losing the placement
/// costs a pill in the wrong corner, not the pill, so failures are logged.
fn place(pill: &WebviewWindow) {
    let monitor = match pill.primary_monitor() {
        Ok(Some(monitor)) => monitor,
        Ok(None) => return,
        Err(error) => {
            crate::host_log::warn(format!(
                "could not find a screen for the call pill: {error}"
            ));
            return;
        }
    };
    let size = match pill.outer_size() {
        Ok(size) => size,
        Err(error) => {
            crate::host_log::warn(format!("could not measure the call pill: {error}"));
            return;
        }
    };
    let area = monitor.work_area();
    let at = first_place(area.position, area.size, size, monitor.scale_factor());
    if let Err(error) = pill.set_position(at) {
        crate::host_log::warn(format!("could not place the call pill: {error}"));
    }
}

/// Whether the pill is on screen: a call is open, and the desktop window is
/// not the one in front. In front, the composer already has the switches.
fn shows(call_open: bool, desktop_in_front: bool) -> bool {
    call_open && !desktop_in_front
}

/// What the desktop window's call looks like now; none when no call is open.
/// The desktop page says this at every change, whichever window is in front.
/// Whether the pill then shows is decided here, from the window itself.
#[tauri::command]
pub fn call_pill_set(
    app: AppHandle,
    window: WebviewWindow,
    pill: State<'_, CallPill>,
    call: Option<CallPicture>,
) -> Result<(), String> {
    only(&window, DESKTOP_WINDOW)?;
    pill.held().picture = call.map(CallPicture::bounded);
    settle(&app, &pill, window.is_focused().unwrap_or(false));
    Ok(())
}

/// The desktop window came to the front or left it.
pub fn desktop_front_changed(app: &AppHandle, pill: &CallPill, in_front: bool) {
    settle(app, pill, in_front);
}

/// The desktop page is loading again: whatever call it held is gone with it.
pub fn desktop_page_reloading(app: &AppHandle, pill: &CallPill) {
    pill.held().picture = None;
    settle(app, pill, true);
}

/// Brings the pill's window in line with the held picture: drawn and on
/// screen, or cleared and hidden. Failures cost the pill, never the call.
fn settle(app: &AppHandle, pill: &CallPill, desktop_in_front: bool) {
    let picture = drawn(pill, desktop_in_front);
    let place_first = picture.is_some() && !std::mem::replace(&mut pill.held().placed, true);
    if let Err(error) = app.emit_to(CALL_WINDOW, host::CALL_PILL_PICTURE, picture.as_ref()) {
        crate::host_log::warn(format!("could not update the call pill: {error}"));
    }
    let Some(native) = app.get_webview_window(CALL_WINDOW) else {
        return;
    };
    let visible = native.is_visible().unwrap_or(false);
    if picture.is_none() {
        if visible {
            if let Err(error) = native.hide() {
                crate::host_log::warn(format!("could not hide the call pill: {error}"));
            }
        }
        return;
    }
    if visible {
        return;
    }
    if place_first {
        place(&native);
    }
    if let Err(error) = crate::platform::current().show_quietly(&native) {
        crate::host_log::warn(format!("could not show the call pill: {error}"));
    }
}

/// What the pill draws now: the held picture while it shows, else none.
fn drawn(pill: &CallPill, desktop_in_front: bool) -> Option<CallPicture> {
    let held = pill.held();
    if shows(held.picture.is_some(), desktop_in_front) {
        held.picture.clone()
    } else {
        None
    }
}

/// The picture the pill should be drawing now; none when no call is shown.
#[tauri::command]
pub fn call_pill_read(
    window: WebviewWindow,
    pill: State<'_, CallPill>,
) -> Result<Option<CallPicture>, String> {
    only(&window, CALL_WINDOW)?;
    let desktop_in_front = window
        .app_handle()
        .get_webview_window(DESKTOP_WINDOW)
        .is_some_and(|desktop| desktop.is_focused().unwrap_or(false));
    Ok(drawn(&pill, desktop_in_front))
}

/// A press on the pill, handed to the desktop window, which owns the call.
#[tauri::command]
pub fn call_pill_press(
    app: AppHandle,
    window: WebviewWindow,
    press: CallPress,
) -> Result<(), String> {
    only(&window, CALL_WINDOW)?;
    if press == CallPress::Open {
        desktop_window::open(&app);
    }
    app.emit_to(DESKTOP_WINDOW, host::CALL_PILL_PRESS, press)
        .map_err(|error| format!("could not reach the call: {error}"))
}

/// Moves the pill by a drag that began on it. The pill has no title bar, so
/// its page asks for the drag; nothing else about the window is the page's.
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

    #[test]
    fn the_pill_shows_only_for_an_open_call_behind_other_windows() {
        assert!(shows(true, false));
        assert!(!shows(true, true));
        assert!(!shows(false, false));
        assert!(!shows(false, true));
    }

    #[test]
    fn the_pill_draws_the_held_picture_only_while_it_shows() {
        let pill = CallPill::default();
        assert_eq!(drawn(&pill, false), None);
        let picture = CallPicture {
            phase: CallPhase::Listening,
            title: "Fix the build".into(),
            microphone_on: true,
            speaker_on: true,
        };
        pill.held().picture = Some(picture.clone());
        assert_eq!(drawn(&pill, false), Some(picture));
        assert_eq!(drawn(&pill, true), None);
    }

    #[test]
    fn the_pill_first_stands_at_the_top_centre_of_the_work_area() {
        let at = first_place(
            PhysicalPosition::new(0, 50),
            PhysicalSize::new(3024, 1900),
            PhysicalSize::new(480, 104),
            2.0,
        );
        assert_eq!(at, PhysicalPosition::new(1272, 70));
    }

    #[test]
    fn a_screen_to_the_left_keeps_its_own_origin() {
        let at = first_place(
            PhysicalPosition::new(-1920, 0),
            PhysicalSize::new(1920, 1080),
            PhysicalSize::new(240, 52),
            1.0,
        );
        assert_eq!(at, PhysicalPosition::new(-1080, 10));
    }

    #[test]
    fn a_work_area_narrower_than_the_pill_starts_it_at_the_edge() {
        let at = first_place(
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(100, 100),
            PhysicalSize::new(240, 52),
            f64::NAN,
        );
        assert_eq!(at, PhysicalPosition::new(0, 10));
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
