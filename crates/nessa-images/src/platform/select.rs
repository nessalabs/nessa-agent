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
