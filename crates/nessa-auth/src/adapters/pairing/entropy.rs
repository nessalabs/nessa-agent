use super::{rand, CryptoRng, RngCore};
use std::num::NonZeroU32;

/// Production OS entropy for the selected crypto library's injected interface.
/// Fallible acquisition is preserved; required infallible calls panic on failure
/// and the physical composition worker reports an unexpected typed fault.
#[derive(Default)]
pub struct OsEntropy;
impl RngCore for OsEntropy {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }
    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.try_fill_bytes(bytes)
            .expect("native OS entropy acquisition failed");
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), rand::Error> {
        getrandom::fill(bytes).map_err(entropy_error)
    }
}
impl CryptoRng for OsEntropy {}

fn entropy_error(_: getrandom::Error) -> rand::Error {
    rand::Error::from(
        NonZeroU32::new(rand::Error::CUSTOM_START).expect("nonzero crypto entropy error code"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_entropy_error_conversion_retains_typed_refusal_and_successful_fill() {
        let refusal = entropy_error(getrandom::Error::UNSUPPORTED);
        assert_eq!(
            refusal.code().map(NonZeroU32::get),
            Some(rand::Error::CUSTOM_START)
        );
        let mut bytes = [0; 8];
        OsEntropy.try_fill_bytes(&mut bytes).unwrap();
    }
}
