//! Immutable, validated values with a meaning shared across domain features.
//! Date preserves calendar precision without inventing a day, time, or timezone.
//!
//! ```text
//! Date --> model knowledge cutoff
//!      --> catalog verification date (requires day precision)
//! Url  --> model documentation source (requires HTTPS)
//! TokenLimits --> published model ceilings / configured execution budgets
//! Sha256Digest --> content a user message refers to without carrying it
//! ImageMediaType --> a user message's images / what a model is published to accept
//! ```
//! Arrows show uses. Each feature adds its own requirements around shared values.

mod date;
pub use date::{Date, DateError};

mod url;
pub use url::{Url, UrlError};

mod token_limits;
pub use token_limits::{TokenLimits, TokenLimitsError};

mod image_media_type;
pub use image_media_type::{ImageMediaType, ImageMediaTypeError};

mod sha256_digest;
pub use sha256_digest::{Sha256Digest, Sha256DigestError};
