//! The running system's own image decoder, where this crate wraps one.
//!
//! ```text
//! normalize ──► own decoders (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, PNM, HDR)
//!     │
//!     └── begins like HEIC, AVIF, JPEG XL, PSD, or camera RAW
//!              │
//!              └──► platform_decoder() ──► macOS: ImageIO, if it agrees on the type
//!                                     └──► elsewhere: none, so refused
//! ```
//!
//! Arrows are fallbacks in order. The crate documentation states the two gates
//! in front of a system decoder; this module holds them. `sniff` is the first
//! and runs everywhere, in front of a test's substitute too. `macos` is the
//! second: its check names system types, so it lives with the system.
#[cfg(target_os = "macos")]
mod macos;
mod select;

pub use select::platform_decoder;
