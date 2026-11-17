/// Stable provider delivery identity, also used to name a trigger binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventId(Box<str>);
/// Empty, oversized or non-ASCII delivery identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidEventId;
impl EventId {
    /// Accept 1–128 ASCII letters, digits or `_-.:`, retaining a compact value.
    pub fn new(value: String) -> Result<Self, InvalidEventId> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
        {
            return Err(InvalidEventId);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the validated provider delivery identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivery_identity_is_bounded_and_has_an_allowed_alphabet() {
        for value in ["", "space here", "a/b", "é", "a\n", &"a".repeat(129)] {
            assert_eq!(EventId::new(value.into()), Err(InvalidEventId));
        }
        for value in ["a", "A-z_1.2:3", &"a".repeat(128)] {
            assert_eq!(EventId::new(value.into()).unwrap().as_str(), value);
        }
    }
    #[test]
    fn an_identity_discards_the_supplied_spare_capacity() {
        let mut input = String::with_capacity(1024 * 1024);
        input.push_str("delivery");
        let compact: Box<str> = EventId::new(input).unwrap().0;
        assert_eq!(&*compact, "delivery");
    }
}
