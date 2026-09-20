//! Which system decoder the running system has, if any.
//!
//! One function, compiled per target. macOS answers with the ImageIO adapter
//! beside this file; every other system answers `None`, which is what makes the
//! encodings only a system reads [`crate::Error::UnsupportedEncoding`] there
//! rather than something guessed at.
#[cfg(target_os = "macos")]
use super::macos::ImageIo;
use crate::PlatformDecoder;

/// The decoder for the running system, or `None` where none is wrapped yet.
pub fn platform_decoder() -> Option<&'static dyn PlatformDecoder> {
    #[cfg(target_os = "macos")]
    {
        Some(&ImageIo)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
