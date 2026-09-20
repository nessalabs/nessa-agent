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
//! Arrows are fallbacks in order. Two gates stand in front of a system decoder.
//! The first is `sniff`, which reads the first bytes and runs on every system,
//! in front of a test's substitute too: anything that does not begin like one
//! of those encodings is refused without asking. The second is the adapter's:
//! `macos` asks ImageIO what it takes the bytes for and decodes only the types
//! on its list, because ImageIO also renders PDFs and much else this crate
//! never said it reads. That check names system types, so it lives with the
//! system. A system without a wrapped decoder refuses those encodings by type;
//! nothing is guessed.
#[cfg(target_os = "macos")]
mod macos;
mod select;

pub use select::platform_decoder;
