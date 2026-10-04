//! Reading the pairing code from standard input.
use super::read_code;
use std::io::Read;

/// A reader that hands out its bytes and then blocks forever, as a terminal
/// does while waiting for more typing.
struct Terminal(&'static [u8]);
impl Read for Terminal {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        assert!(
            !self.0.is_empty(),
            "read past the line: a terminal would block"
        );
        let count = bytes.len().min(self.0.len());
        bytes[..count].copy_from_slice(&self.0[..count]);
        self.0 = &self.0[count..];
        Ok(count)
    }
}

/// Review F6a: the code is one line; nothing past its line ending is read,
/// so an interactive terminal is not waited on until end of input.
#[test]
fn code_is_read_from_one_line() {
    for line in [&b"ABCD-EFGH\n"[..], b"abcd-efgh\r\n", b"ABCDEFGH\n"] {
        let leaked: &'static [u8] = Box::leak(line.to_vec().into_boxed_slice());
        assert!(read_code(&mut Terminal(leaked)).is_some(), "{line:?}");
    }
    assert!(
        read_code(&mut &b"ABCD-EFGH"[..]).is_some(),
        "end of input ends the line"
    );
    for refused in [&b"ABCD-EFG0\n"[..], b"ABCD EFGH\n", b"ABCD-EFGH-X\n", b"\n"] {
        assert!(read_code(&mut &refused[..]).is_none(), "{refused:?}");
    }
}
