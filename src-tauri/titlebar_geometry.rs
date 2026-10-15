//! The page's titlebar height, read from `src/desktop/styles.css`.
//!
//! The page owns the titlebar row: its controls sit on the row's centre. The
//! host has to put the native traffic lights on that same centre, so the build
//! script reads the height from the stylesheet and hands it to the host as
//! `TITLEBAR_HEIGHT` (`platform/macos/traffic_lights.rs`), rather than the
//! host keeping a copy of the number. A change to how the page builds the
//! height fails the build here instead of moving the buttons off the row.

/// The stylesheet, from the desktop crate's directory.
pub const STYLES: &str = "../src/desktop/styles.css";

const EDGE: &str =
    "--desktop-control-edge: calc(var(--desktop-gutter) + var(--desktop-control-inset));";
const HEIGHT: &str = "--desktop-titlebar-height: calc(\n    2 * var(--desktop-control-edge) + var(--desktop-control-size)\n  );";

/// The titlebar's height in px: two control edges (gutter and inset) and a
/// control.
pub fn titlebar_height(css: &str) -> Result<f64, String> {
    // A Windows checkout ends lines with CRLF.
    let css = css.replace("\r\n", "\n");
    if !css.contains(EDGE) || !css.contains(HEIGHT) {
        return Err(String::from(
            "--desktop-titlebar-height is no longer two control edges and a control; \
             update titlebar_geometry.rs to read the new formula",
        ));
    }
    let edge = px(&css, "--desktop-gutter")? + px(&css, "--desktop-control-inset")?;
    Ok(2.0 * edge + px(&css, "--desktop-control-size")?)
}

/// A `--name: 8px;` declaration's number, from the first place it is set.
fn px(css: &str, name: &str) -> Result<f64, String> {
    let start = css
        .find(&format!("{name}:"))
        .ok_or_else(|| format!("styles.css no longer sets {name}"))?;
    let value = &css[start + name.len() + 1..];
    let value = value[..value.find(';').ok_or("a declaration ends")?].trim();
    value
        .strip_suffix("px")
        .and_then(|px| px.parse().ok())
        .ok_or_else(|| format!("{name} is {value}, not a length in px"))
}
