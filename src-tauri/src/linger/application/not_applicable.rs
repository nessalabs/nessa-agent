//! A host with no logind API. Every answer is `not-applicable`, and nothing
//! here can enable linger because the type has no logind port.

use super::session::{LingerOffer, LingerView};

#[derive(Clone, Copy, Debug)]
pub(crate) struct NotApplicableLinger;

impl LingerOffer for NotApplicableLinger {
    fn status(&self) -> LingerView {
        LingerView::not_this_host()
    }

    fn accept(&self) -> LingerView {
        LingerView::not_this_host()
    }

    fn decline(&self) -> LingerView {
        LingerView::not_this_host()
    }
}
