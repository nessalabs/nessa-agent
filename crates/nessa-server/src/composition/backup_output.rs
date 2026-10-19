//! One JSON process edge for the offline backup commands.

use std::io::Write;

use serde::Serialize;

pub(super) fn print<T: Serialize>(output: &mut dyn Write, result: &T, exit: u8) -> u8 {
    let Ok(mut bytes) = serde_json::to_vec(result) else {
        return 1;
    };
    bytes.push(b'\n');
    if output.write_all(&bytes).is_ok() {
        exit
    } else {
        1
    }
}
