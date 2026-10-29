//! Resolve settings from the broadest scope to the most specific scope.

/// A missing choice inherits its parent; a present choice replaces it.
/// `Option<T>` remains a valid explicit value when `T` itself is optional.
pub(crate) fn resolve_inherited<T>(default: T, choices: impl IntoIterator<Item = Option<T>>) -> T {
    let mut effective = default;
    for value in choices.into_iter().flatten() {
        effective = value;
    }
    effective
}

#[cfg(test)]
#[path = "../../tests/settings/inheritance.rs"]
mod tests;
