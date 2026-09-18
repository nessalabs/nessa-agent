//! The menu bar / system tray item.
//!
//! On macOS this is the menu bar extra the panel is summoned from. On Linux
//! it is a StatusNotifierItem when the desktop provides one, and a missing
//! tray is survivable: the panel then lives on the taskbar instead.

use tauri::{
    image::Image,
    menu::{
        CheckMenuItem, CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem,
    },
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Wry,
};

use std::io;

use crate::composition::HostDependencies;
use crate::host;
use crate::panel;
use crate::settings::SettingsStore;
use crate::updater;

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

/// The menu itself, kept because one item is not built with the rest of it.
///
/// The update check finishes after the tray is already on screen — and usually
/// finds nothing — so the item it would add is added to this menu later, or
/// never. See [`offer_update`].
struct TrayMenu(Menu<Wry>);

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
/// Installs the update the background check found and comes back up on it.
/// Only ever in the menu when there is such an update — see [`offer_update`].
const UPDATE_ITEM: &str = "install-update";
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
    let toggle = MenuItemBuilder::with_id("toggle", "Show Nessa").build(app)?;
    let transparent = CheckMenuItemBuilder::with_id("surface", "Transparent")
        .checked(false)
        .build(app)?;
    let stop_agents =
        CheckMenuItemBuilder::with_id("stop-agents-on-quit", "Stop active agents when quitting")
            .checked(deps.settings.load().stop_agents_on_quit)
            .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Nessa").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&toggle, &transparent]);
    #[cfg(debug_assertions)]
    let menu = menu.separator().text(SHOW_SETUP_ITEM, "Show setup again");
    let menu = menu.separator().items(&[&stop_agents, &quit]).build()?;

    app.manage(SurfaceMenuItem(transparent));
    app.manage(QuitPolicyMenuItem(stop_agents));
    app.manage(TrayMenu(menu.clone()));

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
            // The click is the whole of the consent: nothing was downloaded
            // before it, and the restart is named in the item's own text.
            UPDATE_ITEM => updater::install_and_restart(app),
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

/// Puts the update at the top of the menu, above everything the menu already
/// offers, with a separator under it so it reads as its own thing rather than
/// another panel control.
///
/// This is the only thing an available update does. It waits in a menu nobody
/// has to open, and the version is in the text so the click is an informed one.
///
/// A tray that could not be created has no menu to grow, and a menu that
/// refuses the item leaves the app exactly as it was on the version it has:
/// both are reported and survivable, like every other tray failure here.
pub fn offer_update(app: &AppHandle, version: &str) {
    let Some(menu) = app.try_state::<TrayMenu>() else {
        return;
    };

    let item = MenuItemBuilder::with_id(UPDATE_ITEM, format!("Update to {version} and restart"))
        .build(app);
    let placed = item.and_then(|item| {
        let separator = PredefinedMenuItem::separator(app)?;
        menu.0.prepend_items(&[&item, &separator])
    });
    if let Err(error) = placed {
        eprintln!("[nessa] could not offer the update in the tray: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
