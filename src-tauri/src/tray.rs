//! The menu bar / system tray item.
//!
//! On macOS this is the menu bar extra the panel is summoned from. On Linux
//! it is a StatusNotifierItem when the desktop provides one, and a missing
//! tray is survivable: the panel then lives on the taskbar instead.

use tauri::{
    image::Image,
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, Wry,
};

use serde::Serialize;
use std::io;
use std::sync::Arc;

use crate::composition::HostDependencies;
use crate::desktop_window;
use crate::host;
use crate::panel;
use crate::settings::SettingsStore;

/// The surface control lives in the tray menu rather than in the panel's
/// header: the header is only two lines tall and the panel is dismissed by the
/// tray icon anyway, so neither control earned a permanent seat there.
///
/// The frontend stays the source of truth — it remembers the choice across
/// launches — and this item is kept in step by `set_frosted`.
pub struct SurfaceMenuItem(pub CheckMenuItem<Wry>);

/// Whether the tray item exists. Close-to-hide only makes sense when it does.
pub struct Present(pub bool);
struct QuitPolicyMenuItem(CheckMenuItem<Wry>);

const TRAY_ID: &str = "nessa-tray";
/// Reopens first-run setup, and un-finishes it: `panel::restart_onboarding`
/// clears the persisted completion as well as showing the window, so this is a
/// real restart rather than a second look at a setup the file still calls done.
///
/// Debug builds only: setup now runs once and records that it did, so this is
/// the only way to see it again while working on it. It is not a feature
/// anybody asked for, so it does not ship until it is one.
#[cfg(debug_assertions)]
const SHOW_SETUP_ITEM: &str = "show-setup";
/// The menu bar icon, compiled in rather than resolved as a bundle resource so
/// dev and packaged builds load the identical bytes with no path lookup.
///
/// It is a *different* painting from the app icon: same seed and hue wheel, but
/// heavier pigment. The app icon's pastel washes sit around 0.87–0.97 lightness,
/// which at the 16pt the menu bar actually draws collapses into a pale disc —
/// a white blob on any bar. Delicate reads fine in the Dock at 128pt; the menu
/// bar needs contrast. See the README on regenerating it.
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-icon.png");

/// Flips the quit policy in the settings file and answers with the value that
/// actually reached the disk.
///
/// Through [`SettingsStore::update`], not a load-change-save: a settings file
/// that failed to parse must not be replaced by the defaults this toggle
/// happened to flip. The tick follows what was written, not what the click
/// assumed it flipped — the file this same call read may not say what the menu
/// was showing.
fn toggle_quit_policy(settings: &dyn SettingsStore) -> io::Result<bool> {
    settings
        .update(&mut |chosen| chosen.stop_agents_on_quit = !chosen.stop_agents_on_quit)
        .map(|written| written.stop_agents_on_quit)
}

/// Builds the tray item and its menu.
///
/// `deps` is the host's dependency bundle, resolved once by `main`'s `setup` and
/// handed on: the menu's initial tick comes from it, and the menu-event handler
/// captures it rather than reaching back through the app for what it needs.
pub fn create(app: &AppHandle, deps: &HostDependencies) -> tauri::Result<()> {
    let open_window = MenuItemBuilder::with_id("open-window", "Open Nessa").build(app)?;
    let toggle = MenuItemBuilder::with_id("toggle", "Show Panel").build(app)?;
    let transparent = CheckMenuItemBuilder::with_id("surface", "Transparent")
        .checked(false)
        .build(app)?;
    let stop_agents =
        CheckMenuItemBuilder::with_id("stop-agents-on-quit", "Stop active agents when quitting")
            .checked(deps.settings.load().stop_agents_on_quit)
            .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Nessa").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&open_window, &toggle, &transparent]);
    #[cfg(debug_assertions)]
    let menu = menu.separator().text(SHOW_SETUP_ITEM, "Show setup again");
    let menu = menu.separator().items(&[&stop_agents, &quit]).build()?;

    app.manage(SurfaceMenuItem(transparent));
    app.manage(QuitPolicyMenuItem(stop_agents));

    // Captured, not looked up: the handler is built here, where the bundle is
    // already in hand, so nothing inside it has to ask the app for a dependency.
    let deps = deps.clone();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // The menu belongs to the right button; the left button summons the
        // panel, which is what a menu bar app is for.
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            // The tray is not teaching the shortcut, so it has no use for
            // which way the panel went.
            "open-window" => desktop_window::open(app),
            "toggle" => {
                panel::toggle(app);
            }
            // The frontend owns the choice, so the click is only a request.
            "surface" => {
                let _ = app.emit(host::TOGGLE_SURFACE, ());
            }
            // The rule is in `toggle_quit_policy`; all that is left here is
            // moving the tick to whatever was written.
            "stop-agents-on-quit" => match toggle_quit_policy(&*deps.settings) {
                Ok(stop_agents) => {
                    if let Some(item) = app.try_state::<QuitPolicyMenuItem>() {
                        let _ = item.0.set_checked(stop_agents);
                    }
                }
                Err(error) => eprintln!("[nessa] could not save settings: {error}"),
            },
            #[cfg(debug_assertions)]
            SHOW_SETUP_ITEM => panel::restart_onboarding(app, &*deps.settings),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                panel::toggle(tray.app_handle());
            }
        });

    match Image::from_bytes(TRAY_ICON) {
        Ok(icon) => builder = builder.icon(icon),
        // The app icon is the wrong weight for a menu bar, but an icon that is
        // hard to see beats a tray item with none at all.
        Err(error) => {
            eprintln!("[nessa] falling back to the app icon in the tray: {error}");
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
        }
    }

    builder.build(app)?;
    Ok(())
}

/// Where the menu bar icon is, in the setup window's own CSS pixels.
///
/// The finished setup window gathers into a small light and flies here, so it
/// lands on the icon it becomes rather than somewhere near it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TrayPoint {
    pub x: f64,
    pub y: f64,
}

/// Where the menu bar icon and the setup window are, as the window server
/// reports them: the icon's centre and the window's page origin in physical
/// screen pixels, and the window's scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayLayout {
    pub centre: (f64, f64),
    pub origin: (f64, f64),
    pub scale: f64,
}

/// Asks the window server where the menu bar icon and the setup window are.
///
/// The window server is outside the process, so it is reached through this
/// seam (`HostDependencies::tray_geometry`) and a test can stand in for it.
pub trait TrayGeometry: Send + Sync {
    /// `Ok(None)` when there is nothing to measure: no tray, no setup window,
    /// or a system that does not say where its tray items are (Linux reports
    /// no rect). `Err` when the window server refused a question it was asked.
    fn layout(&self) -> Result<Option<TrayLayout>, String>;
}

/// The window server Tauri talks to.
struct NativeTrayGeometry(AppHandle);

impl TrayGeometry for NativeTrayGeometry {
    fn layout(&self) -> Result<Option<TrayLayout>, String> {
        let Some(tray) = self.0.tray_by_id(TRAY_ID) else {
            return Ok(None);
        };
        let Some(rect) = tray.rect().map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        let Some(window) = self.0.get_webview_window(panel::SETUP_WINDOW) else {
            return Ok(None);
        };
        let scale = window.scale_factor().map_err(|error| error.to_string())?;
        let origin = window.inner_position().map_err(|error| error.to_string())?;
        let position = rect.position.to_physical::<f64>(scale);
        let size = rect.size.to_physical::<f64>(scale);
        Ok(Some(TrayLayout {
            centre: (
                position.x + size.width / 2.0,
                position.y + size.height / 2.0,
            ),
            origin: (f64::from(origin.x), f64::from(origin.y)),
            scale,
        }))
    }
}

/// The tray geometry this app's window server answers with.
pub fn tray_geometry(app: &AppHandle) -> Arc<dyn TrayGeometry> {
    Arc::new(NativeTrayGeometry(app.clone()))
}

/// The menu bar icon's centre, as a point in the setup window's page.
///
/// None when there is nothing to aim at, when the window server refused, or
/// when what it said cannot be a place on screen. The page then aims at the
/// top right corner, where the icon is on every desktop this runs on.
#[tauri::command]
pub fn setup_tray_point(deps: State<'_, HostDependencies>) -> Option<TrayPoint> {
    aim(deps.tray_geometry.as_ref())
}

fn aim(geometry: &dyn TrayGeometry) -> Option<TrayPoint> {
    match geometry.layout() {
        Ok(layout) => {
            layout.and_then(|layout| tray_point(layout.centre, layout.origin, layout.scale))
        }
        Err(error) => {
            tracing::warn!(target: "nessa.setup", %error, "could not read where the tray icon is");
            None
        }
    }
}

/// A point on the screen in physical pixels, as CSS pixels from the page's
/// top left corner.
fn tray_point(centre: (f64, f64), origin: (f64, f64), scale: f64) -> Option<TrayPoint> {
    let finite = [centre.0, centre.1, origin.0, origin.1, scale]
        .iter()
        .all(|value| value.is_finite());
    if !finite || scale <= 0.0 {
        return None;
    }
    Some(TrayPoint {
        x: (centre.0 - origin.0) / scale,
        y: (centre.1 - origin.1) / scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_point_is_measured_from_the_page_in_css_pixels() {
        // A Retina screen: the icon's centre at (2800, 22) physical, the page
        // at the screen's origin.
        assert_eq!(
            tray_point((2800.0, 22.0), (0.0, 0.0), 2.0),
            Some(TrayPoint { x: 1400.0, y: 11.0 })
        );
        // A window that does not start at the screen's origin.
        assert_eq!(
            tray_point((1300.0, 12.0), (100.0, -20.0), 1.0),
            Some(TrayPoint { x: 1200.0, y: 32.0 })
        );
    }

    #[test]
    fn no_tray_point_is_made_up_from_a_broken_scale() {
        assert_eq!(tray_point((10.0, 10.0), (0.0, 0.0), 0.0), None);
        assert_eq!(tray_point((f64::NAN, 10.0), (0.0, 0.0), 1.0), None);
    }

    struct Answers(Result<Option<TrayLayout>, String>);

    impl TrayGeometry for Answers {
        fn layout(&self) -> Result<Option<TrayLayout>, String> {
            self.0.clone()
        }
    }

    #[test]
    fn the_command_aims_at_what_the_window_server_reports() {
        let reported = Answers(Ok(Some(TrayLayout {
            centre: (2800.0, 22.0),
            origin: (0.0, 0.0),
            scale: 2.0,
        })));
        assert_eq!(aim(&reported), Some(TrayPoint { x: 1400.0, y: 11.0 }));
    }

    #[test]
    fn an_absent_tray_or_window_gives_no_point() {
        assert_eq!(aim(&Answers(Ok(None))), None);
    }

    #[test]
    fn a_refused_read_gives_no_point() {
        assert_eq!(aim(&Answers(Err("no window server".into()))), None);
    }

    #[test]
    fn a_malformed_layout_gives_no_point() {
        for layout in [
            TrayLayout {
                centre: (10.0, 10.0),
                origin: (0.0, 0.0),
                scale: -1.0,
            },
            TrayLayout {
                centre: (10.0, f64::INFINITY),
                origin: (0.0, 0.0),
                scale: 1.0,
            },
            TrayLayout {
                centre: (10.0, 10.0),
                origin: (f64::NAN, 0.0),
                scale: 1.0,
            },
        ] {
            assert_eq!(aim(&Answers(Ok(Some(layout)))), None);
        }
    }
    use crate::settings::testing::in_memory;

    /// What the item ticks is what reached the disk. Previously this rule could
    /// only be watched by clicking a real menu bar item.
    #[test]
    fn the_quit_policy_toggle_reports_the_value_it_wrote() {
        let settings = in_memory();

        assert!(toggle_quit_policy(&settings.store).expect("an absent file takes the change"));
        assert!(settings.store.load().stop_agents_on_quit);

        assert!(!toggle_quit_policy(&settings.store).expect("a readable file takes the change"));
        assert!(!settings.store.load().stop_agents_on_quit);
    }

    /// The toggle flips what the file says, not what the menu was showing: a
    /// file changed by hand since launch wins, because it is the durable one.
    #[test]
    fn the_toggle_flips_what_the_file_says_rather_than_what_the_menu_shows() {
        let settings = in_memory();
        settings
            .storage
            .put(&settings.path, br#"{"stopAgentsOnQuit":true}"#);

        assert!(!toggle_quit_policy(&settings.store).expect("a readable file takes the change"));
    }

    /// A settings file this build cannot parse must not be replaced by the
    /// defaults this toggle happened to flip. The click costs a diagnostic line
    /// and the tick stays where it was.
    #[test]
    fn a_malformed_settings_file_is_refused_and_its_bytes_are_left_alone() {
        let settings = in_memory();
        let original = br#"{ "stopAgentsOnQuit": true, "#.to_vec();
        settings.storage.put(&settings.path, &original);

        let refused = toggle_quit_policy(&settings.store)
            .expect_err("defaults are not a basis for replacing a file that failed to parse");

        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
        assert_eq!(settings.storage.get(&settings.path), Some(original));
    }

    /// Keys the toggle does not touch survive it: the panel geometry somebody
    /// dragged into place is not the quit policy's to overwrite.
    #[test]
    fn the_toggle_keeps_the_keys_it_did_not_change() {
        let settings = in_memory();
        settings.storage.put(
            &settings.path,
            br#"{"panel":{"width":640,"minWidth":500},"onboarding":{"completed":true}}"#,
        );

        assert!(toggle_quit_policy(&settings.store).expect("a readable file takes the change"));

        let saved = settings.store.load();
        assert!(saved.stop_agents_on_quit);
        assert_eq!(saved.panel.width, 640.0);
        assert_eq!(saved.panel.min_width, 500.0);
        assert!(saved.onboarding.completed);
    }
}
