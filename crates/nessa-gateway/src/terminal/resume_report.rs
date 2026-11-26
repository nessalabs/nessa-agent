//! OSC 7577 (Program Resume Protocol, rev 0.5): one report a program sends
//! to say how to start it again after the terminal restarts.
//!
//! `ESC ] 7577 ; pid=<pid>:time=<secs>:argv=<b64>,<b64>…[:cwd=<b64>]:mac=<hex> ST`,
//! or `clear=1` in place of `argv`. `mac` is the SHA-256 of the body before
//! `:mac=`, then `:`, then the terminal's token (`OSC7577_TOKEN`, which only
//! the terminal's first process was given). The checks run in the spec's
//! order, and each failure has the spec's outcome:
//!
//! - **Ignored** (changes nothing): a missing, malformed or wrong `mac`, a
//!   missing or bad `time`, or a `time` more than [`TIME_WINDOW_SECS`] from
//!   the gateway's clock. Text a program merely printed (`cat`, a log tail,
//!   ssh output) never carries the token, so it lands here.
//! - **Rejected** (forgets the owner's record): a report that passed `mac`
//!   and `time` but breaks any other rule — a repeated key, a bad pid, both
//!   or neither of `argv` and `clear`, a relative path, a control character,
//!   or a `need` naming a key this terminal does not implement.
//! - **Accepted**: the report becomes the terminal's record.
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use subtle::ConstantTimeEq;

/// The environment variable that carries a terminal's token, and says the
/// terminal takes OSC 7577 reports.
pub const TOKEN_VAR: &str = "OSC7577_TOKEN";
/// How far a report's `time` may be from the gateway's clock, in seconds.
pub const TIME_WINDOW_SECS: u64 = 60;
/// The most `argv` elements a report may carry (the spec's floor is 64).
pub const MAX_ARGV: usize = 256;
/// The keys this terminal implements, which a `need` may name.
const IMPLEMENTED: [&str; 7] = ["pid", "time", "mac", "argv", "cwd", "clear", "need"];

/// The command a record restarts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeCommand {
    /// The program, then its arguments; each one argument, never shell text.
    pub argv: Vec<String>,
    /// The folder it runs in, when the program named one (absolute).
    pub cwd: Option<String>,
}

/// What a report asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportAction {
    Resume(ResumeCommand),
    Clear,
}

/// A report that passed every check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The process the record belongs to.
    pub pid: u32,
    pub time: u64,
    pub action: ReportAction,
}

/// The outcome of one report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ignored,
    /// Forget the records of `pid`; `None` when its pid was unreadable.
    Rejected {
        pid: Option<u32>,
    },
    Accepted(Report),
}

/// A fresh token for one terminal: 64 characters from 256 random bits.
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Check one OSC 7577 body (the text after `7577;`) against `token` at
/// `now_secs`. A report the program built is answered in the spec's order:
/// the `mac`, then the `time`, then everything else.
pub fn verify(body: &str, token: &str, now_secs: u64) -> Verdict {
    if body == "?" {
        return Verdict::Ignored;
    }
    let Some((signed, mac)) = body.rsplit_once(":mac=") else {
        return Verdict::Ignored;
    };
    if mac.len() != 64 || !mac.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Verdict::Ignored;
    }
    let mut hasher = Sha256::new();
    hasher.update(signed.as_bytes());
    hasher.update(b":");
    hasher.update(token.as_bytes());
    let expected = hex(&hasher.finalize());
    if !bool::from(expected.as_bytes().ct_eq(mac.as_bytes())) {
        return Verdict::Ignored;
    }
    // The time is checked before the rest: a stale or replayed report is
    // ignored, never rejected, so it cannot forget a newer record.
    let times: Vec<&str> = signed
        .split(':')
        .filter_map(|pair| pair.strip_prefix("time="))
        .collect();
    let [time] = times.as_slice() else {
        return Verdict::Ignored;
    };
    let Some(time) = positive(time) else {
        return Verdict::Ignored;
    };
    if time.abs_diff(now_secs) > TIME_WINDOW_SECS {
        return Verdict::Ignored;
    }
    let mut seen = HashSet::new();
    let mut pid = None;
    let mut argv = None;
    let mut cwd = None;
    let mut clear = None;
    let mut need = None;
    let mut malformed = false;
    for pair in signed.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            malformed = true;
            continue;
        };
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value.bytes().all(is_value_byte)
            || !seen.insert(key)
        {
            malformed = true;
            continue;
        }
        match key {
            "pid" => pid = Some(value),
            "argv" => argv = Some(value),
            "cwd" => cwd = Some(value),
            "clear" => clear = Some(value),
            "need" => need = Some(value),
            // `time` was read above; any other key is one this terminal
            // does not know, and is ignored unless `need` names it.
            _ => {}
        }
    }
    let pid = pid
        .and_then(positive)
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| i32::try_from(*pid).is_ok());
    let rejected = Verdict::Rejected { pid };
    let Some(pid) = pid else {
        return rejected;
    };
    if malformed {
        return rejected;
    }
    if let Some(need) = need {
        if need.is_empty() || need.split(',').any(|name| !IMPLEMENTED.contains(&name)) {
            return rejected;
        }
    }
    let action = match (argv, clear) {
        (Some(argv), None) => {
            let Some(argv) = decode_argv(argv) else {
                return rejected;
            };
            let cwd = match cwd {
                None => None,
                Some(cwd) => match decode_text(cwd) {
                    Some(cwd) if cwd.starts_with('/') => Some(cwd),
                    _ => return rejected,
                },
            };
            ReportAction::Resume(ResumeCommand { argv, cwd })
        }
        (None, Some("1")) => {
            if cwd.is_some() {
                // A clear carries no command, so a folder means nothing.
                return rejected;
            }
            ReportAction::Clear
        }
        _ => return rejected,
    };
    Verdict::Accepted(Report { pid, time, action })
}

/// `[1-9][0-9]*` as a number; `None` when it is not one, or too large.
fn positive(text: &str) -> Option<u64> {
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn is_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

/// The command: at least one element, the first a bare name or an absolute
/// path, never relative and never an option.
fn decode_argv(value: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() > MAX_ARGV {
        return None;
    }
    let argv = parts
        .into_iter()
        .map(decode_text)
        .collect::<Option<Vec<String>>>()?;
    let program = argv.first()?;
    let bare = !program.contains('/');
    let valid = if bare {
        !program.is_empty() && !program.starts_with('-')
    } else {
        program.starts_with('/')
    };
    valid.then_some(argv)
}

/// Standard base64 of UTF-8 with no control character. Padding is
/// optional; any padding present must be right.
fn decode_text(encoded: &str) -> Option<String> {
    let unpadded = encoded.trim_end_matches('=');
    let padding = encoded.len() - unpadded.len();
    if padding > 2 || unpadded.contains('=') {
        return None;
    }
    if padding > 0 && !encoded.len().is_multiple_of(4) {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(unpadded.as_bytes())
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A command as text a person can read safely: each element quoted when it
/// needs it, and every control, bidi or invisible character shown as an
/// escape, so a name cannot pretend to be another.
pub fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|part| {
            let escaped: String = part
                .chars()
                .map(|ch| {
                    if ch.is_control() || is_format_char(ch) {
                        format!("\\u{{{:04x}}}", ch as u32)
                    } else {
                        ch.to_string()
                    }
                })
                .collect();
            let plain = !escaped.is_empty()
                && escaped
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || "-_./=+,:@%".contains(ch));
            if plain {
                escaped
            } else {
                format!("'{}'", escaped.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bidi overrides and isolates, and invisible formatting characters.
fn is_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "dummy-token-replace-me";

    fn b64(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    fn signed(body: &str, token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        hasher.update(b":");
        hasher.update(token.as_bytes());
        format!("{body}:mac={}", hex(&hasher.finalize()))
    }

    #[test]
    fn matches_the_spec_test_vector() {
        assert_eq!(
            signed("pid=74877:time=1795543200:clear=1", TOKEN),
            "pid=74877:time=1795543200:clear=1:mac=c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93"
        );
        assert_eq!(
            verify(
                &signed("pid=74877:time=1795543200:clear=1", TOKEN),
                TOKEN,
                1_795_543_200
            ),
            Verdict::Accepted(Report {
                pid: 74877,
                time: 1_795_543_200,
                action: ReportAction::Clear
            })
        );
    }

    #[test]
    fn accepts_a_command_with_its_folder() {
        let body = format!(
            "pid=42:time=1000:argv={},{},{}:cwd={}",
            b64("nvim"),
            b64("-S"),
            b64("/home/me/.local/state/nvim/s1.vim"),
            b64("/home/me/project")
        );
        assert_eq!(
            verify(&signed(&body, TOKEN), TOKEN, 1010),
            Verdict::Accepted(Report {
                pid: 42,
                time: 1000,
                action: ReportAction::Resume(ResumeCommand {
                    argv: vec![
                        "nvim".into(),
                        "-S".into(),
                        "/home/me/.local/state/nvim/s1.vim".into()
                    ],
                    cwd: Some("/home/me/project".into()),
                })
            })
        );
    }

    #[test]
    fn ignores_what_lacks_the_token_or_the_time() {
        let body = format!("pid=42:time=1000:argv={}", b64("nvim"));
        // Wrong token, no mac, mac not last, upper-case hex.
        assert_eq!(
            verify(&signed(&body, "other"), TOKEN, 1000),
            Verdict::Ignored
        );
        assert_eq!(verify(&body, TOKEN, 1000), Verdict::Ignored);
        let moved = signed(&body, TOKEN).replacen("pid=42:", "", 1) + ":pid=42";
        assert_eq!(verify(&moved, TOKEN, 1000), Verdict::Ignored);
        assert_eq!(
            verify(&signed(&body, TOKEN).to_uppercase(), TOKEN, 1000),
            Verdict::Ignored
        );
        // Out of the window either way, missing, repeated, or zero-led.
        assert_eq!(verify(&signed(&body, TOKEN), TOKEN, 1061), Verdict::Ignored);
        assert_eq!(verify(&signed(&body, TOKEN), TOKEN, 939), Verdict::Ignored);
        let timeless = format!("pid=42:argv={}", b64("nvim"));
        assert_eq!(
            verify(&signed(&timeless, TOKEN), TOKEN, 1000),
            Verdict::Ignored
        );
        let twice = format!("pid=42:time=1000:time=1000:argv={}", b64("nvim"));
        assert_eq!(
            verify(&signed(&twice, TOKEN), TOKEN, 1000),
            Verdict::Ignored
        );
        let led = format!("pid=42:time=01000:argv={}", b64("nvim"));
        assert_eq!(verify(&signed(&led, TOKEN), TOKEN, 1000), Verdict::Ignored);
        let huge = format!("pid=42:time=99999999999999999999999:argv={}", b64("nvim"));
        assert_eq!(verify(&signed(&huge, TOKEN), TOKEN, 1000), Verdict::Ignored);
        assert_eq!(verify("?", TOKEN, 1000), Verdict::Ignored);
    }

    #[test]
    fn rejects_what_breaks_a_rule_after_the_mac_and_time() {
        let rejected = |body: &str| verify(&signed(body, TOKEN), TOKEN, 1000);
        let at = |pid| Verdict::Rejected { pid };
        // A relative program or folder, an option as the program.
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={}", b64("bin/nvim"))),
            at(Some(42))
        );
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={}", b64("-rf"))),
            at(Some(42))
        );
        assert_eq!(
            rejected(&format!(
                "pid=42:time=1000:argv={}:cwd={}",
                b64("nvim"),
                b64("project")
            )),
            at(Some(42))
        );
        // Both, or neither, of argv and clear; clear not 1; an empty argv.
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={}:clear=1", b64("nvim"))),
            at(Some(42))
        );
        assert_eq!(rejected("pid=42:time=1000"), at(Some(42)));
        assert_eq!(rejected("pid=42:time=1000:clear=2"), at(Some(42)));
        assert_eq!(rejected("pid=42:time=1000:argv="), at(Some(42)));
        // A need this terminal lacks; one it has is fine.
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={}:need=env", b64("nvim"))),
            at(Some(42))
        );
        assert!(matches!(
            rejected(&format!("pid=42:time=1000:argv={}:need=cwd", b64("nvim"))),
            Verdict::Accepted(_)
        ));
        // A repeated key, a bad pair, a control character, bad padding.
        assert_eq!(
            rejected(&format!("pid=42:pid=42:time=1000:argv={}", b64("nvim"))),
            at(Some(42))
        );
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={}:oops", b64("nvim"))),
            at(Some(42))
        );
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={}", b64("nvim\nrm"))),
            at(Some(42))
        );
        assert_eq!(rejected("pid=42:time=1000:argv=bnZpbQ="), at(Some(42)));
        // An unreadable pid forgets with no pid to name.
        assert_eq!(
            rejected(&format!("pid=0:time=1000:argv={}", b64("nvim"))),
            at(None)
        );
        // Unknown keys are ignored; empty later elements are kept.
        assert_eq!(
            rejected(&format!("pid=42:time=1000:argv={},:future=x", b64("nvim"))),
            Verdict::Accepted(Report {
                pid: 42,
                time: 1000,
                action: ReportAction::Resume(ResumeCommand {
                    argv: vec!["nvim".into(), String::new()],
                    cwd: None
                })
            })
        );
    }

    #[test]
    fn accepts_what_a_shell_script_signs() {
        // `verification/desktop/scripts/fixtures/resume-probe.sh kept w1`,
        // run with this token: base64 and sha256sum from a POSIX shell.
        let body = "pid=3416:time=1795659447:argv=L2hvbWUvdXNlci9uZXNzYS1hZ2VudC92ZXJpZmljYXRpb24vZGVza3RvcC9zY3JpcHRzL2ZpeHR1cmVzL3Jlc3VtZS1wcm9iZS5zaA==,cmVzdW1lZA==,dzE=:cwd=L3RtcC9jbGF1ZGUtMA==:mac=50807a6dad748be97cbbebe40a302131aa681170a4aedf091f703f10f7c1c54d";
        assert_eq!(
            verify(body, "abcdefghijklmnopqrstuvwx", 1_795_659_450),
            Verdict::Accepted(Report {
                pid: 3416,
                time: 1_795_659_447,
                action: ReportAction::Resume(ResumeCommand {
                    argv: vec![
                        "/home/user/nessa-agent/verification/desktop/scripts/fixtures/resume-probe.sh".into(),
                        "resumed".into(),
                        "w1".into()
                    ],
                    cwd: Some("/tmp/claude-0".into()),
                })
            })
        );
    }

    #[test]
    fn shows_a_command_with_nothing_hidden() {
        assert_eq!(
            display_command(&["nvim".into(), "-S".into(), "/tmp/a b".into()]),
            "nvim -S '/tmp/a b'"
        );
        assert_eq!(
            display_command(&["ls".into(), "\u{202e}txt.exe".into()]),
            "ls '\\u{202e}txt.exe'"
        );
        assert_eq!(new_token().len(), 64);
    }
}
