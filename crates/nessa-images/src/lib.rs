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
//! Only bytes that begin like one of those encodings are ever put to a system
//! decoder, and the macOS adapter decodes only what ImageIO itself names as one
//! of them: a PDF, an archive, or text is refused by type, never rendered.
//!
//! ```text
//! bytes ──► sniff encoding ──► (begins like a system encoding? ──► platform
//!                │               decoder ──► scale ► encode; anything else
//!                │               this crate does not read is refused)
//!                ▼
//!           read size and orientation ──► too many pixels, or too much
//!                                   │     memory to work in? refused
//!                                   │
//!              already acceptable? ─┴─ yes ──► decodes whole? ──► the same
//!                                   │                             bytes, untouched
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
//!
//! Memory is bounded before any pixel is read, for every decoder alike. An
//! image of more than one hundred million pixels is
//! [`Error::TooLargeToDecode`], and so is one whose decoded pixels and working
//! copies, counted from its header, would pass 768 MiB: a hundred-megapixel
//! photograph at eight bits a channel fits, the same size in sixteen bits or
//! floating point does not. A system decoder is held to the same pixel count
//! from the file's own header, and is never asked for an edge over 8,192 px.
//!
//! Passing through is not taking a header's word. The image is decoded whole
//! first, a JPEG by a strict decoder because the forgiving one used for fitting
//! shows a file cut short as its top half, and only then are the original bytes
//! returned. Strictness decides that one thing: a JPEG the strict decoder refuses
//! may still be a picture every viewer shows, so it is read by the forgiving one
//! and written again, and what neither can read is [`Error::Undecodable`], as is
//! any other encoding that is truncated or damaged. GIF is the limit of that
//! promise: it carries no checksum, so damaged pixel data that still reads as
//! codes is an image as far as any decoder can tell. An animation is never
//! handed back as it came: its first frame is written again.
#![deny(missing_docs)]

mod budget;
mod decode;
mod error;
mod jpeg;
mod limits;
mod normalize;
mod platform;
mod sniff;

pub use decode::{DecodedImage, PlatformDecoder};
pub use error::Error;
pub use limits::{Encoding, Limits, LimitsError};
pub use normalize::{normalize, normalize_with, Normalized};
pub use platform::platform_decoder;
