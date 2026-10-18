//! Percent-decoding at the persisted backup representation boundary.

pub(super) fn decode(value: &str) -> Result<String, ()> {
    decode_bounded(value, usize::MAX)
}

pub(super) fn decode_bounded(value: &str, max_bytes: usize) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if out.len() >= max_bytes {
            return Err(());
        }
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(());
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).map_err(|_| ())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}
