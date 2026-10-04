//! Whether text is one JSON value (RFC 8259), checked without building it.
//!
//! The domain may not depend on a serialization library, and only needs a
//! yes or no, so this walks the text once with an explicit stack: nesting
//! depth costs heap, never the call stack, whatever the input.

/// What an open container expects next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    /// An array: a value, or `]` right after `[`.
    ArrayStart,
    /// An array after a value: `,` or `]`.
    ArrayNext,
    /// An object: a key, or `}` right after `{`.
    ObjectStart,
    /// An object after a value: `,` or `}`.
    ObjectNext,
}

/// Whether `text` is exactly one JSON value, surrounded by optional whitespace.
pub(in crate::domain::agent_execution) fn is_json(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut open: Vec<Open> = Vec::new();
    loop {
        at = space(bytes, at);
        // A value is expected here, unless a container closes empty.
        match (open.last().copied(), bytes.get(at)) {
            (Some(Open::ArrayStart), Some(b']')) | (Some(Open::ObjectStart), Some(b'}')) => {
                open.pop();
                at += 1;
            }
            (_, Some(b'[')) => {
                open.push(Open::ArrayStart);
                at += 1;
                continue;
            }
            (_, Some(b'{')) => {
                at += 1;
                match key(bytes, at) {
                    // Past its first key, an object closes only after a value.
                    Some(next) => {
                        open.push(Open::ObjectNext);
                        at = next;
                    }
                    None if matches!(bytes.get(space(bytes, at)), Some(b'}')) => {
                        open.push(Open::ObjectStart);
                        continue;
                    }
                    None => return false,
                }
                continue;
            }
            (_, _) => match scalar(bytes, at) {
                Some(next) => at = next,
                None => return false,
            },
        }
        // A value has ended: close containers or move to the next element. An
        // object here is always past a key (`ObjectStart` closes only empty).
        loop {
            at = space(bytes, at);
            let Some(top) = open.last_mut() else {
                return at == bytes.len();
            };
            match (*top, bytes.get(at)) {
                (Open::ArrayStart | Open::ArrayNext, Some(b',')) => {
                    *top = Open::ArrayNext;
                    at += 1;
                    break;
                }
                (Open::ObjectNext, Some(b',')) => {
                    match key(bytes, at + 1) {
                        Some(next) => at = next,
                        None => return false,
                    }
                    break;
                }
                (Open::ArrayStart | Open::ArrayNext, Some(b']'))
                | (Open::ObjectNext, Some(b'}')) => {
                    open.pop();
                    at += 1;
                }
                _ => return false,
            }
        }
    }
}

fn space(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// A key and its colon, from `at`: the position after the colon.
fn key(bytes: &[u8], at: usize) -> Option<usize> {
    let at = space(bytes, at);
    let at = string(bytes, at)?;
    let at = space(bytes, at);
    (bytes.get(at) == Some(&b':')).then_some(at + 1)
}

/// A string, number, `true`, `false` or `null` from `at`: the position after it.
fn scalar(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'"' => string(bytes, at),
        b'-' | b'0'..=b'9' => number(bytes, at),
        _ => [&b"true"[..], b"false", b"null"]
            .into_iter()
            .find(|word| bytes[at..].starts_with(word))
            .map(|word| at + word.len()),
    }
}

fn string(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    let mut at = at + 1;
    loop {
        match *bytes.get(at)? {
            b'"' => return Some(at + 1),
            b'\\' => match *bytes.get(at + 1)? {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => at += 2,
                b'u' => {
                    let hex = bytes.get(at + 2..at + 6)?;
                    if !hex.iter().all(u8::is_ascii_hexdigit) {
                        return None;
                    }
                    at += 6;
                }
                _ => return None,
            },
            // Control characters must be escaped; the text is already UTF-8.
            0x00..=0x1f => return None,
            _ => at += 1,
        }
    }
}

fn number(bytes: &[u8], mut at: usize) -> Option<usize> {
    // How many digits run from `at`, and the same when at least one is required.
    let run = |at: usize| {
        bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let digits = |at: usize| match run(at) {
        0 => None,
        count => Some(at + count),
    };
    if bytes.get(at) == Some(&b'-') {
        at += 1;
    }
    at = match bytes.get(at)? {
        b'0' => at + 1,
        b'1'..=b'9' => at + run(at),
        _ => return None,
    };
    if bytes.get(at) == Some(&b'.') {
        at = digits(at + 1)?;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        at = digits(at)?;
    }
    Some(at)
}
