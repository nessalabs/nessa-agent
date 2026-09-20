//! The running system's own image decoder, where this crate wraps one.
//!
//! ```text
//! normalize ──► own decoders (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, TGA, QOI, PNM, HDR)
//!     │
//!     └── not one of those ──► platform_decoder() ──► macOS: ImageIO
//!                                                └──► elsewhere: none, so refused
//! ```
//!
//! Arrows are fallbacks in order. A system without a wrapped decoder refuses
//! those encodings by type, exactly as before; nothing is guessed.
use crate::PlatformDecoder;

#[cfg(target_os = "macos")]
mod macos;

/// The decoder for the running system, or `None` where none is wrapped yet.
pub fn platform_decoder() -> Option<&'static dyn PlatformDecoder> {
    #[cfg(target_os = "macos")]
    {
        Some(&macos::ImageIo)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
