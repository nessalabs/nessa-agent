/// `Self` is how a view shows `T`: one SDK value read as one view value.
///
/// `From` cannot carry these, since neither side is this crate's type.
pub trait ViewOf<T> {
    /// `value` as a view shows it.
    fn view_of(value: T) -> Self;
}

/// [`ViewOf`] spelled from the value's side, as `.into()` is for `From`.
pub trait IntoView<V> {
    /// `self` as a view shows it.
    fn into_view(self) -> V;
}

impl<T, V: ViewOf<T>> IntoView<V> for T {
    fn into_view(self) -> V {
        V::view_of(self)
    }
}
