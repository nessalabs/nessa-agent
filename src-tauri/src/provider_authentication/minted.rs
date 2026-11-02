//! The token `claude setup-token` prints, read from what its terminal shows.
//!
//! The CLI draws a terminal interface: it moves the cursor rather than
//! printing spaces, so every escape sequence is read as a separator and the
//! token is the unbroken run of token characters after its prefix.

const PREFIX: &str = "sk-ant-oat";

/// The last token shown in `screen`, if one is.
pub(crate) fn token_in(screen: &[u8]) -> Option<String> {
    let text = plain(screen);
    let start = text.rfind(PREFIX)?;
    let token: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (token.len() > PREFIX.len() + 8).then_some(token)
}

/// `screen` with every escape sequence and control character a space.
fn plain(screen: &[u8]) -> String {
    let text = String::from_utf8_lossy(screen);
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                match chars.next() {
                    // CSI: parameters, then one final byte in @..~.
                    Some('[') => {
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    // OSC: up to BEL or ESC \.
                    Some(']') => {
                        while let Some(c) = chars.next() {
                            if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some())
                            {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
                plain.push(' ');
            }
            c if c.is_control() => plain.push(' '),
            c => plain.push(c),
        }
    }
    plain
}

#[cfg(test)]
#[path = "../../tests/provider_authentication/minted.rs"]
mod tests;
