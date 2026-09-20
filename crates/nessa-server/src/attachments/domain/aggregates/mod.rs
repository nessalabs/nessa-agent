//! Consistency boundaries: every outstanding ticket answers to one book.
mod ticket_book;

pub use ticket_book::{BookFull, Redemption, TicketBook};
