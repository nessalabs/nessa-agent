use std::{error::Error, fmt};

/// The text is not `sha256:` followed by 64 lowercase hexadecimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha256DigestError;
impl fmt::Display for Sha256DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "digest must be `sha256:` and 64 lowercase hexadecimal digits"
        )
    }
}
impl Error for Sha256DigestError {}

/// Immutable SHA-256 content digest. It names bytes; it does not hold or hash them.
///
/// The one text form is `sha256:<64 lowercase hex>`, so two spellings of one
/// digest cannot compare unequal. The feature that uses a digest decides what
/// content it names and who computed it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sha256Digest([u8; 32]);
impl Sha256Digest {
    const PREFIX: &'static str = "sha256:";

    /// Wrap the 32 bytes a SHA-256 computation produced.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse the canonical text form. Uppercase digits, a missing prefix, and
    /// any other length are rejected rather than normalized.
    pub fn parse(value: &str) -> Result<Self, Sha256DigestError> {
        let hex = value.strip_prefix(Self::PREFIX).ok_or(Sha256DigestError)?;
        if hex.len() != 64 {
            return Err(Sha256DigestError);
        }
        let mut bytes = [0_u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }

    /// Borrow the raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The 64 lowercase hexadecimal digits without the algorithm prefix: a
    /// portable file-name form.
    pub fn to_hex(&self) -> String {
        use fmt::Write;
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }
}

fn nibble(digit: u8) -> Result<u8, Sha256DigestError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(Sha256DigestError),
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.to_hex())
    }
}
impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256Digest({self})")
    }
}
