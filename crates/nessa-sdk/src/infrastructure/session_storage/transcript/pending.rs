//! Immutable pending semantic bytes; the physical validator owns their bounds.
use std::{mem, sync::Arc};

#[derive(Clone, Default)]
pub(super) struct PendingBody {
    tail: Option<Arc<Piece>>,
    bytes: usize,
    pieces: usize,
}
struct Piece {
    previous: Option<Arc<Piece>>,
    bytes: Box<[u8]>,
}
impl Drop for Piece {
    fn drop(&mut self) {
        let mut previous = self.previous.take();
        while let Some(piece) = previous {
            match Arc::try_unwrap(piece) {
                Ok(mut piece) => previous = piece.previous.take(),
                Err(_) => break,
            }
        }
    }
}
impl PendingBody {
    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.tail = Some(Arc::new(Piece {
            previous: self.tail.clone(),
            bytes: bytes.into(),
        }));
        self.bytes += bytes.len();
        self.pieces += 1;
    }
    pub(super) fn assemble(&self) -> Vec<u8> {
        let mut pieces = Vec::with_capacity(self.pieces);
        let mut next = self.tail.as_deref();
        while let Some(piece) = next {
            pieces.push(piece.bytes.as_ref());
            next = piece.previous.as_deref();
        }
        let mut body = Vec::with_capacity(self.bytes);
        for piece in pieces.into_iter().rev() {
            body.extend_from_slice(piece);
        }
        body
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.bytes.saturating_add(
            self.pieces
                .saturating_mul(mem::size_of::<Piece>() + 2 * mem::size_of::<usize>()),
        )
    }
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_piece_staging_copies_each_piece_once_and_shares_old_tails() {
        let mut body = PendingBody::default();
        let mut copied = 0;
        for index in 0usize..4096 {
            let old = body.clone();
            let bytes = index.to_be_bytes();
            body.push(&bytes);
            copied += bytes.len();
            if let Some(tail) = old.tail.as_ref() {
                assert!(Arc::ptr_eq(
                    tail,
                    body.tail.as_ref().unwrap().previous.as_ref().unwrap()
                ));
            }
            assert_eq!(body.bytes, copied);
            assert_eq!(body.pieces, index + 1);
        }
        let saved = body.clone();
        body.clear();
        assert_eq!(saved.assemble().len(), copied);
        assert_eq!(
            saved.retained_bytes(),
            copied + 4096 * (mem::size_of::<Piece>() + 2 * mem::size_of::<usize>())
        );
        drop(saved);
    }
}
