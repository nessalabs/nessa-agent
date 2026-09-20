//! Fit one image to a consumer's limits.
//!
//! Give [`normalize`] the bytes of an image and the [`Limits`] of whatever will
//! receive it. It answers with bytes that are inside every limit, in an encoding
//! the consumer accepts, or with a typed reason it could not.
//!
//! This crate knows nothing about agents, models, conversations, or where limits
//! come from. It reads no files, opens no sockets, and keeps no state. The caller
//! owns the numbers.
//!
//! It reads PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, PNM, and Radiance HDR with
//! its own decoders, and the same bytes and limits always give the same answer.
//! HEIC and HEIF, AVIF, JPEG XL, PSD, and camera RAW have no permissively
//! licensed Rust decoder, and operating systems ship good ones, so those go to a
//! [`PlatformDecoder`]: ImageIO on macOS, none yet elsewhere, where they are
//! refused by type. What a system decoder returns is that system's rendering.
//!
//! ```text
//! bytes ──► sniff encoding ──► (not ours? ──► platform decoder ──► scale ► encode)
//!                │
//!                ▼
//!           read size and orientation
//!                                   │
//!              already acceptable? ─┴─ yes ──► the same bytes, untouched
//!                                   │
//!                                   no
//!                                   ▼
//!        decode first frame ► turn upright ► scale down ► encode ► fits? ──► bytes
//!                                               ▲                    │
//!                                               └── smaller ◄── no ──┘
//! ```
//!
//! Arrows are the order of work. The loop lowers JPEG quality a little, then
//! scales the image down, and gives up with [`Error::CannotFit`] rather than
//! return something unreadably small.
//!
//! Three rules shape the output:
//!
//! - **Leave a good image alone.** Bytes already inside every limit pass through
//!   unchanged. Re-encoding costs quality, most of all for text in screenshots,
//!   and every lossy pass compounds.
//! - **Never trust a label.** The encoding is read from the bytes. A declared
//!   media type is not an input.
//! - **Show what the person saw.** A camera records rotation beside the pixels,
//!   and consumers that ignore metadata see such a photo sideways, so rotation is
//!   applied to the pixels. Only the first frame of an animation is kept.
//!
//! Output is PNG or JPEG. PNG is tried first for anything that was lossless or
//! has transparency, because those are usually screenshots and diagrams; JPEG is
//! the fallback when PNG cannot fit, with transparency flattened onto white.
//!
//! An image that passes through keeps whatever metadata it carried. One that is
//! re-encoded carries none.
#![deny(missing_docs)]

mod decode;
mod error;
mod limits;
mod normalize;
mod platform;

pub use decode::{DecodedImage, PlatformDecoder};
pub use error::Error;
pub use limits::{Encoding, Limits, LimitsError};
pub use normalize::{normalize, normalize_with, Normalized};
pub use platform::platform_decoder;
