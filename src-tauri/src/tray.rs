//! The menu bar / system tray item.
//!
//! On macOS this is the menu bar extra the panel is summoned from. On Linux
//! it is a StatusNotifierItem when the desktop provides one, and a missing
//! tray is survivable: the panel then lives on the taskbar instead.

use tauri::{
    image::Image,
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Wry,
};

use crate::host;
use crate::panel;

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
/// The menu bar icon, compiled in rather than resolved as a bundle resource so
/// dev and packaged builds load the identical bytes with no path lookup.
///
/// It is a *different* painting from the app icon: same seed and hue wheel, but
/// heavier pigment. The app icon's pastel washes sit around 0.87–0.97 lightness,
/// which at the 16pt the menu bar actually draws collapses into a pale disc —
/// a white blob on any bar. Delicate reads fine in the Dock at 128pt; the menu
/// bar needs contrast. See the README on regenerating it.
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-icon.png");

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItemBuilder::with_id("toggle", "Show Nessa").build(app)?;
    let transparent = CheckMenuItemBuilder::with_id("surface", "Transparent")
        .checked(false)
        .build(app)?;
    let stop_agents =
        CheckMenuItemBuilder::with_id("stop-agents-on-quit", "Stop active agents when quitting")
            .checked(crate::settings::load(app).stop_agents_on_quit)
            .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Nessa").build(app)?;
    // First-run setup is not persisted yet, so it runs on every launch and
    // there is no way to see it twice in one. This runs it on demand, which is
    // what makes it possible to work on at all.
    let setup = MenuItemBuilder::with_id("restart-onboarding", "Restart onboarding").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&toggle, &transparent])
        .separator()
        .items(&[&setup])
        .separator()
        .items(&[&stop_agents, &quit])
        .build()?;

    app.manage(SurfaceMenuItem(transparent));
    app.manage(QuitPolicyMenuItem(stop_agents));

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // The menu belongs to the right button; the left button summons the
        // panel, which is what a menu bar app is for.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            // The tray is not teaching the shortcut, so it has no use for
            // which way the panel went.
            "toggle" => {
                panel::toggle(app);
            }
            // The frontend owns the choice, so the click is only a request.
            "surface" => {
                let _ = app.emit(host::TOGGLE_SURFACE, ());
            }
            "stop-agents-on-quit" => {
                let mut settings = crate::settings::load(app);
                settings.stop_agents_on_quit = !settings.stop_agents_on_quit;
                match crate::settings::save(app, &settings) {
                    Ok(()) => {
                        if let Some(item) = app.try_state::<QuitPolicyMenuItem>() {
                            let _ = item.0.set_checked(settings.stop_agents_on_quit);
                        }
                    }
                    Err(error) => eprintln!("[nessa] could not save settings: {error}"),
                }
            }
            "restart-onboarding" => crate::panel::restart_onboarding(app),
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
