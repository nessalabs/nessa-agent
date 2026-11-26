//! OSC 7501 (Program Status Protocol, rev 0.4) parser, record store, and
//! streaming VT scanner for terminal output.
//!
//! A terminal program reports its state in-band over the PTY:
//! `ESC ] 7501 ; state=<state>[:<key>=<value>]... ST`
//! where `ST` is `ESC \` or `BEL`.
//!
//! Feature detection: `ESC ] 7501 ; ? ST` is answered with
//! `ESC ] 7501 ; ?:states=idle,working,done,blocked,error:kinds=permission,question,auth ESC \`.
//!
//! Record lifetime (rev 0.4):
//! - `working` and `blocked` drop on process exit or `OSC 133;A` (shell prompt,
//!   excluding `redraw=0` prompt redraws).
//! - `done`, `error`, and `idle` survive process exit and `OSC 133;A` until replaced or cleared.
//! - `RIS` (`ESC c`) clears every record; `DECSTR` and alt-screen switches clear nothing.
//! - Up to 256 records are retained per terminal with LRU eviction.
//!
//! The same scan hands OSC 7577 (program resume) bodies on, unchecked, in
//! `ScanOutput::resume_reports`; `resume_tracker` decides what they mean.
//!
//! When no OSC 7501 records are active, OSC 0 / OSC 2 window titles are also
//! inspected solely to recognize Claude Code or Codex while open.

use base64::Engine;
use std::collections::HashMap;

/// Maximum total byte length of a single OSC 7501 sequence (including `ESC ]` and `ST`).
pub const MAX_SEQUENCE_BYTES: usize = 4096;
/// Maximum byte length of a single key name.
const MAX_KEY_BYTES: usize = 16;
/// Maximum byte length of an `id` path.
const MAX_ID_BYTES: usize = 128;
/// Maximum byte length of a single `id` segment or `app` name.
const MAX_SEGMENT_BYTES: usize = 32;
/// Maximum depth (segment count) of an `id` path.
const MAX_ID_DEPTH: usize = 8;
/// Maximum encoded / decoded byte lengths for `title` and `msg`.
const MAX_TITLE_ENCODED_BYTES: usize = 256;
const MAX_TITLE_DECODED_BYTES: usize = 192;
const MAX_MSG_ENCODED_BYTES: usize = 2732;
const MAX_MSG_DECODED_BYTES: usize = 2048;
/// Maximum number of concurrent records retained per terminal.
pub const MAX_RECORDS: usize = 256;

/// Fixed feature-detection reply sent to the PTY in response to `OSC 7501 ; ? ST`.
pub const FEATURE_DETECTION_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// Program state reported via OSC 7501.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramStatusState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl ProgramStatusState {
    /// Urgency rank for selecting which active record to surface:
    /// `blocked > error > working > done > idle`.
    fn priority(self) -> u8 {
        match self {
            Self::Blocked => 5,
            Self::Error => 4,
            Self::Working => 3,
            Self::Done => 2,
            Self::Idle => 1,
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }
}

/// Reason a program is `blocked` waiting on the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramBlockedKind {
    Permission,
    Question,
    Auth,
}

impl ProgramBlockedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// The single most urgent active program status for a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatus {
    pub state: ProgramStatusState,
    pub kind: Option<ProgramBlockedKind>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

/// One stored OSC 7501 record, keyed by its hierarchical `id` (`""` for root).
#[derive(Clone, Debug)]
struct Record {
    state: ProgramStatusState,
    kind: Option<ProgramBlockedKind>,
    progress: Option<u8>,
    app: Option<String>,
    title: Option<String>,
    msg: Option<String>,
    seq: u64,
}

/// What a single `feed` call produced.
#[derive(Debug, Default)]
pub struct ScanOutput {
    /// Reply bytes to write back to the PTY (at most one feature-detection reply per feed).
    pub replies: Vec<u8>,
    /// `true` when the effective `ProgramStatus` changed.
    pub changed: bool,
    /// The bodies of OSC 7577 (program resume) reports, in order, for the
    /// terminal's resume tracker to check. At most [`MAX_RESUME_REPORTS`].
    pub resume_reports: Vec<String>,
}

/// The most OSC 7577 reports kept from one feed; more are dropped.
pub const MAX_RESUME_REPORTS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParseState {
    Normal,
    Esc,
    Osc,
    OscEsc,
}

/// Stateful VT stream scanner and OSC 7501 record store for one terminal.
pub struct ProgramStatusTracker {
    parse: ParseState,
    buf: Vec<u8>,
    overflowed: bool,
    records: HashMap<String, Record>,
    next_seq: u64,
    /// Fallback Claude/Codex detection from OSC 0 / OSC 2 window title when no
    /// OSC 7501 records are active.
    title_app: Option<String>,
    /// Last published effective status, used to detect real changes.
    current: Option<ProgramStatus>,
}

impl Default for ProgramStatusTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgramStatusTracker {
    pub fn new() -> Self {
        Self {
            parse: ParseState::Normal,
            buf: Vec::new(),
            overflowed: false,
            records: HashMap::new(),
            next_seq: 0,
            title_app: None,
            current: None,
        }
    }

    /// Current effective status, if any.
    pub fn current(&self) -> Option<&ProgramStatus> {
        self.current.as_ref()
    }

    /// Feed a chunk of raw PTY output bytes.
    pub fn feed(&mut self, bytes: &[u8]) -> ScanOutput {
        let mut out = ScanOutput::default();
        for &byte in bytes {
            match self.parse {
                ParseState::Normal => {
                    if byte == 0x1b {
                        self.parse = ParseState::Esc;
                    }
                }
                ParseState::Esc => match byte {
                    b']' => {
                        self.parse = ParseState::Osc;
                        self.buf.clear();
                        self.overflowed = false;
                    }
                    b'c' => {
                        // RIS (Full Reset): clears every record.
                        self.parse = ParseState::Normal;
                        self.records.clear();
                        self.title_app = None;
                    }
                    0x1b => {
                        self.parse = ParseState::Esc;
                    }
                    _ => {
                        self.parse = ParseState::Normal;
                    }
                },
                ParseState::Osc => match byte {
                    0x07 => {
                        // BEL terminates OSC (1-byte terminator).
                        self.finish_osc(1, &mut out);
                        self.parse = ParseState::Normal;
                    }
                    0x1b => {
                        self.parse = ParseState::OscEsc;
                    }
                    0x18 | 0x1a => {
                        // CAN / SUB aborts the current control string.
                        self.buf.clear();
                        self.overflowed = false;
                        self.parse = ParseState::Normal;
                    }
                    _ => {
                        // Reserve 2 bytes for `ESC ]` and at least 1 byte for `ST` (BEL).
                        if !self.overflowed {
                            if self.buf.len() < MAX_SEQUENCE_BYTES - 3 {
                                self.buf.push(byte);
                            } else {
                                self.overflowed = true;
                                self.buf.clear();
                            }
                        }
                    }
                },
                ParseState::OscEsc => match byte {
                    b'\\' => {
                        // ESC \ terminates OSC (2-byte terminator).
                        self.finish_osc(2, &mut out);
                        self.parse = ParseState::Normal;
                    }
                    b']' => {
                        // Aborted previous OSC, new OSC begins.
                        self.buf.clear();
                        self.overflowed = false;
                        self.parse = ParseState::Osc;
                    }
                    b'c' => {
                        // Aborted previous OSC, RIS full reset.
                        self.buf.clear();
                        self.overflowed = false;
                        self.records.clear();
                        self.title_app = None;
                        self.parse = ParseState::Normal;
                    }
                    0x1b => {
                        self.buf.clear();
                        self.overflowed = false;
                        self.parse = ParseState::Esc;
                    }
                    _ => {
                        self.buf.clear();
                        self.overflowed = false;
                        self.parse = ParseState::Normal;
                    }
                },
            }
        }
        out.changed = self.refresh_current();
        out
    }

    /// Apply process-exit lifetime rules: drop `working` and `blocked`, keep `done`/`error`/`idle`.
    /// Returns `true` if the effective status changed.
    pub fn on_exit(&mut self) -> bool {
        self.drop_live_states();
        self.title_app = None;
        self.refresh_current()
    }

    fn finish_osc(&mut self, st_len: usize, out: &mut ScanOutput) {
        if self.overflowed {
            self.overflowed = false;
            self.buf.clear();
            return;
        }
        let total_len = 2 + self.buf.len() + st_len;
        let payload = std::mem::take(&mut self.buf);
        let Some(semi) = payload.iter().position(|&b| b == b';') else {
            return;
        };
        let code = &payload[..semi];
        let body = &payload[semi + 1..];
        match code {
            b"7501" => {
                if total_len > MAX_SEQUENCE_BYTES {
                    return;
                }
                if body == b"?" {
                    // Emit at most one reply per feed chunk to prevent reply amplification.
                    if out.replies.is_empty() {
                        out.replies.extend_from_slice(FEATURE_DETECTION_REPLY);
                    }
                } else if let Ok(text) = std::str::from_utf8(body) {
                    self.apply_osc7501(text);
                }
            }
            b"7577" => {
                if total_len <= MAX_SEQUENCE_BYTES && out.resume_reports.len() < MAX_RESUME_REPORTS
                {
                    if let Ok(text) = std::str::from_utf8(body) {
                        out.resume_reports.push(text.to_owned());
                    }
                }
            }
            b"133" => {
                // OSC 133;A (shell prompt start) drops live working/blocked states and stale
                // fallback title detection, except on prompt redraws (`redraw=0`).
                if is_shell_prompt_start(body) {
                    self.drop_live_states();
                    self.title_app = None;
                }
            }
            b"0" | b"2" => {
                if let Ok(title) = std::str::from_utf8(body) {
                    self.title_app = detect_title_app(title);
                }
            }
            _ => {}
        }
    }

    fn drop_live_states(&mut self) {
        self.records.retain(|_, record| !record.state.is_live());
    }

    fn apply_osc7501(&mut self, body: &str) {
        let Some(parsed) = parse_osc7501_body(body) else {
            return;
        };
        match parsed {
            ParsedReport::Clear { id } => {
                if id.is_empty() {
                    self.records.clear();
                    self.title_app = None;
                } else {
                    let prefix = format!("{id}/");
                    self.records
                        .retain(|key, _| key != &id && !key.starts_with(&prefix));
                }
            }
            ParsedReport::Set { id, record } => {
                self.next_seq = self.next_seq.wrapping_add(1);
                let seq = self.next_seq;
                if !self.records.contains_key(&id) && self.records.len() >= MAX_RECORDS {
                    if let Some(oldest_key) = self
                        .records
                        .iter()
                        .min_by_key(|(_, r)| r.seq)
                        .map(|(k, _)| k.clone())
                    {
                        self.records.remove(&oldest_key);
                    }
                }
                self.records.insert(
                    id,
                    Record {
                        state: record.state,
                        kind: record.kind,
                        progress: record.progress,
                        app: record.app,
                        title: record.title,
                        msg: record.msg,
                        seq,
                    },
                );
            }
        }
    }

    fn compute_effective(&self) -> Option<ProgramStatus> {
        if let Some((best_id, best)) = self
            .records
            .iter()
            .max_by_key(|(_, record)| (record.state.priority(), record.seq))
        {
            // Inherit `app` strictly from OSC 7501 ancestors, never from stale title fallback.
            let app = best.app.clone().or_else(|| self.inherited_app(best_id));
            return Some(ProgramStatus {
                state: best.state,
                kind: best.kind,
                progress: best.progress,
                app,
                title: best.title.clone(),
                msg: best.msg.clone(),
            });
        }
        self.title_app.as_ref().map(|app| ProgramStatus {
            state: ProgramStatusState::Idle,
            kind: None,
            progress: None,
            app: Some(app.clone()),
            title: None,
            msg: None,
        })
    }

    fn inherited_app(&self, id: &str) -> Option<String> {
        let mut cursor = id;
        while !cursor.is_empty() {
            cursor = match cursor.rfind('/') {
                Some(idx) => &cursor[..idx],
                None => "",
            };
            if let Some(app) = self.records.get(cursor).and_then(|r| r.app.as_ref()) {
                return Some(app.clone());
            }
        }
        None
    }

    fn refresh_current(&mut self) -> bool {
        let next = self.compute_effective();
        if next != self.current {
            self.current = next;
            true
        } else {
            false
        }
    }
}

fn is_shell_prompt_start(body: &[u8]) -> bool {
    if body == b"A" {
        return true;
    }
    let Some(rest) = body.strip_prefix(b"A;") else {
        return false;
    };
    !rest.split(|&b| b == b';').any(|param| param == b"redraw=0")
}

enum ParsedReport {
    Clear { id: String },
    Set { id: String, record: ProgramStatus },
}

#[derive(Clone, Copy)]
enum RawState {
    Status(ProgramStatusState),
    Clear,
}

fn parse_osc7501_body(body: &str) -> Option<ParsedReport> {
    if body.is_empty() {
        return None;
    }
    let mut state: Option<RawState> = None;
    let mut id: Option<String> = None;
    let mut kind: Option<ProgramBlockedKind> = None;
    let mut invalid_kind = false;
    let mut progress: Option<u8> = None;
    let mut invalid_progress = false;
    let mut app: Option<String> = None;
    let mut title: Option<String> = None;
    let mut msg: Option<String> = None;

    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            // Malformed pair without `=` is skipped per spec.
            continue;
        };
        if key.len() > MAX_KEY_BYTES {
            // Key limit violation discards the whole report.
            return None;
        }
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            // Malformed key is skipped.
            continue;
        }
        if !value.bytes().all(is_value_byte) {
            if is_known_key(key) {
                return None;
            }
            continue;
        }
        // Validate every occurrence of a known key so a later duplicate cannot mask
        // an earlier limit violation or invalid value.
        match key {
            "state" => {
                state = Some(match value {
                    "idle" => RawState::Status(ProgramStatusState::Idle),
                    "working" => RawState::Status(ProgramStatusState::Working),
                    "done" => RawState::Status(ProgramStatusState::Done),
                    "blocked" => RawState::Status(ProgramStatusState::Blocked),
                    "error" => RawState::Status(ProgramStatusState::Error),
                    "clear" => RawState::Clear,
                    _ => return None,
                });
            }
            "id" => {
                id = Some(validate_id(value)?.to_owned());
            }
            "kind" => match value {
                "permission" => kind = Some(ProgramBlockedKind::Permission),
                "question" => kind = Some(ProgramBlockedKind::Question),
                "auth" => kind = Some(ProgramBlockedKind::Auth),
                _ => invalid_kind = true,
            },
            "progress" => {
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(num) = value.parse::<u8>() {
                        if num <= 100 {
                            progress = Some(num);
                            continue;
                        }
                    }
                }
                invalid_progress = true;
            }
            "app" => {
                if !is_valid_segment(value) {
                    return None;
                }
                app = Some(value.to_owned());
            }
            "title" => {
                title = decode_b64_text(value, MAX_TITLE_ENCODED_BYTES, MAX_TITLE_DECODED_BYTES)?;
            }
            "msg" => {
                msg = decode_b64_text(value, MAX_MSG_ENCODED_BYTES, MAX_MSG_DECODED_BYTES)?;
            }
            // Unknown keys are ignored per spec.
            _ => {}
        }
    }

    let id = id.unwrap_or_default();
    let RawState::Status(state) = state? else {
        return Some(ParsedReport::Clear { id });
    };

    let kind = if state == ProgramStatusState::Blocked {
        if invalid_kind {
            return None;
        }
        kind
    } else {
        None
    };

    let progress = if state.is_live() {
        if invalid_progress {
            return None;
        }
        progress
    } else {
        None
    };

    Some(ParsedReport::Set {
        id,
        record: ProgramStatus {
            state,
            kind,
            progress,
            app,
            title,
            msg,
        },
    })
}

fn is_known_key(key: &str) -> bool {
    matches!(
        key,
        "state" | "id" | "kind" | "progress" | "app" | "title" | "msg"
    )
}

fn is_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

fn is_segment_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
}

fn is_valid_segment(segment: &str) -> bool {
    (1..=MAX_SEGMENT_BYTES).contains(&segment.len()) && segment.bytes().all(is_segment_byte)
}

fn validate_id(id: &str) -> Option<&str> {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return None;
    }
    let mut depth = 0usize;
    for segment in id.split('/') {
        depth += 1;
        if depth > MAX_ID_DEPTH || !is_valid_segment(segment) {
            return None;
        }
    }
    Some(id)
}

/// Decode base64 UTF-8 text (padding optional), enforcing encoded/decoded byte
/// limits, rejecting control characters and line separators, and stripping
/// bidi/invisible controls. Returns `Some(None)` when the sanitized text is
/// empty, or `None` when the input is invalid.
fn decode_b64_text(
    encoded: &str,
    max_encoded: usize,
    max_decoded: usize,
) -> Option<Option<String>> {
    if encoded.len() > max_encoded {
        return None;
    }
    // Accept standard base64 with or without `=` padding.
    let unpadded = encoded.trim_end_matches('=');
    let pad_len = encoded.len() - unpadded.len();
    if pad_len > 2 || unpadded.contains('=') {
        return None;
    }
    if pad_len > 0 && !encoded.len().is_multiple_of(4) {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(unpadded.as_bytes())
        .ok()?;
    if bytes.len() > max_decoded {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return None;
    }
    let sanitized: String = text
        .chars()
        .filter(|&ch| !is_disarmed_format_char(ch))
        .collect();
    if sanitized.is_empty() {
        Some(None)
    } else {
        Some(Some(sanitized))
    }
}

/// Bidi overrides, isolates, and invisible formatting characters disarmed per spec.
fn is_disarmed_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}' // SOFT HYPHEN
        | '\u{061C}' // ARABIC LETTER MARK
        | '\u{200B}'..='\u{200F}' // ZWSP, ZWNJ, ZWJ, LRM, RLM
        | '\u{202A}'..='\u{202E}' // LRE, RLE, PDF, LRO, RLO
        | '\u{2060}'..='\u{2064}' // WORD JOINER and invisible operators
        | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
        | '\u{FEFF}' // BOM / ZWNBSP
    )
}

/// Recognize Claude Code or Codex from an OSC 0 / OSC 2 terminal title when no
/// OSC 7501 records are active. Matches only known agent title forms (with
/// optional leading spinner glyph) so ordinary shell paths or editor titles
/// like `~/projects/claude` or `vim codex.ts` do not false-positive.
fn detect_title_app(title: &str) -> Option<String> {
    let trimmed = title.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\\') {
        return None;
    }
    // Strip optional leading non-ASCII status/spinner glyphs (e.g. `✳ `, `⠂ `).
    let stripped = trimmed
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && !c.is_ascii_whitespace())
        .trim_start();
    let lower = stripped.to_ascii_lowercase();
    if lower == "claude"
        || lower == "claude code"
        || lower == "claude-code"
        || lower.starts_with("claude code ")
        || lower.starts_with("claude code:")
        || lower.starts_with("claude code -")
        || lower.starts_with("claude code —")
        || lower.ends_with(" - claude code")
        || lower.ends_with(" — claude code")
    {
        return Some("claude-code".to_owned());
    }
    if lower == "codex"
        || lower == "openai-codex"
        || lower.starts_with("codex ")
        || lower.starts_with("codex:")
        || lower.starts_with("codex -")
        || lower.starts_with("codex —")
        || lower.ends_with(" - codex")
        || lower.ends_with(" — codex")
    {
        return Some("codex".to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    fn b64(text: &str) -> String {
        STANDARD.encode(text.as_bytes())
    }

    #[test]
    fn answers_feature_detection_probe_with_bel_or_st_and_caps_burst() {
        let mut tracker = ProgramStatusTracker::new();
        let out = tracker.feed(b"\x1b]7501;?\x1b\\");
        assert_eq!(out.replies, FEATURE_DETECTION_REPLY);
        assert!(!out.changed);

        let out = tracker.feed(b"\x1b]7501;?\x07");
        assert_eq!(out.replies, FEATURE_DETECTION_REPLY);
        assert!(!out.changed);

        // Multiple probes in a single chunk produce at most one reply.
        let out = tracker.feed(b"\x1b]7501;?\x07\x1b]7501;?\x07\x1b]7501;?\x07");
        assert_eq!(out.replies, FEATURE_DETECTION_REPLY);
    }

    #[test]
    fn hands_on_resume_reports_split_across_chunks_with_either_terminator() {
        let mut tracker = ProgramStatusTracker::new();
        let stream = b"ls\x1b]7577;pid=1:time=2:clear=1:mac=ab\x1b\\x\x1b]7577;?\x07";
        let mut reports = Vec::new();
        for chunk in stream.chunks(3) {
            reports.extend(tracker.feed(chunk).resume_reports);
        }
        assert_eq!(
            reports,
            vec!["pid=1:time=2:clear=1:mac=ab".to_owned(), "?".to_owned()]
        );
        let oversized = format!("\x1b]7577;pid=1:x={}\x07", "a".repeat(4096));
        assert!(tracker.feed(oversized.as_bytes()).resume_reports.is_empty());
        let many = "\x1b]7577;x\x07".repeat(MAX_RESUME_REPORTS + 4);
        assert_eq!(
            tracker.feed(many.as_bytes()).resume_reports.len(),
            MAX_RESUME_REPORTS
        );
    }

    #[test]
    fn parses_report_split_across_arbitrary_byte_chunks() {
        let mut tracker = ProgramStatusTracker::new();
        let seq = format!(
            "\x1b]7501;state=blocked:kind=permission:app=claude-code:msg={}\x1b\\",
            b64("Allow running cargo test?")
        );
        let mut changed = false;
        for byte in seq.as_bytes() {
            let out = tracker.feed(&[*byte]);
            changed |= out.changed;
        }
        assert!(changed);
        let status = tracker.current().expect("status set");
        assert_eq!(status.state, ProgramStatusState::Blocked);
        assert_eq!(status.kind, Some(ProgramBlockedKind::Permission));
        assert_eq!(status.app.as_deref(), Some("claude-code"));
        assert_eq!(status.msg.as_deref(), Some("Allow running cargo test?"));
    }

    #[test]
    fn selects_most_urgent_record_and_inherits_ancestor_app() {
        let mut tracker = ProgramStatusTracker::new();
        tracker.feed(b"\x1b]7501;state=working:app=claude-code\x1b\\");
        tracker.feed(
            format!(
                "\x1b]7501;state=working:id=sub/task:progress=40:title={}\x1b\\",
                b64("Build")
            )
            .as_bytes(),
        );
        let s = tracker.current().unwrap();
        assert_eq!(s.state, ProgramStatusState::Working);
        assert_eq!(s.progress, Some(40));
        assert_eq!(s.app.as_deref(), Some("claude-code"));

        // Blocked outranks working, and inherits app from root.
        tracker.feed(
            format!(
                "\x1b]7501;state=blocked:id=sub/approval:kind=question:msg={}\x1b\\",
                b64("Proceed?")
            )
            .as_bytes(),
        );
        let s = tracker.current().unwrap();
        assert_eq!(s.state, ProgramStatusState::Blocked);
        assert_eq!(s.kind, Some(ProgramBlockedKind::Question));
        assert_eq!(s.app.as_deref(), Some("claude-code"));
        assert_eq!(s.msg.as_deref(), Some("Proceed?"));

        // Clearing `sub` removes both `sub/task` and `sub/approval`, leaving root.
        tracker.feed(b"\x1b]7501;state=clear:id=sub\x1b\\");
        let s = tracker.current().unwrap();
        assert_eq!(s.state, ProgramStatusState::Working);
        assert_eq!(s.progress, None);
        assert_eq!(s.app.as_deref(), Some("claude-code"));

        // Global clear removes everything.
        tracker.feed(b"\x1b]7501;state=clear\x1b\\");
        assert!(tracker.current().is_none());
    }

    #[test]
    fn lifetime_rules_on_osc133_redraw_exit_and_ris() {
        let mut tracker = ProgramStatusTracker::new();
        tracker.feed(b"\x1b]7501;state=working:app=claude-code\x1b\\");
        tracker.feed(b"\x1b]7501;state=done:id=build:msg=ZG9uZQ\x1b\\");
        assert_eq!(
            tracker.current().unwrap().state,
            ProgramStatusState::Working
        );

        // OSC 133;A;redraw=0 is a prompt redraw and must NOT drop working/blocked.
        let out = tracker.feed(b"\x1b]133;A;redraw=0\x07");
        assert!(!out.changed);
        assert_eq!(
            tracker.current().unwrap().state,
            ProgramStatusState::Working
        );

        // Regular OSC 133;A drops working/blocked, keeps done.
        let out = tracker.feed(b"\x1b]133;A\x07");
        assert!(out.changed);
        let s = tracker.current().unwrap();
        assert_eq!(s.state, ProgramStatusState::Done);
        assert_eq!(s.msg.as_deref(), Some("done"));

        // Process exit drops working/blocked, keeps done/error/idle.
        tracker.feed(b"\x1b]7501;state=blocked:id=ask:kind=auth\x1b\\");
        assert_eq!(
            tracker.current().unwrap().state,
            ProgramStatusState::Blocked
        );
        assert!(tracker.on_exit());
        assert_eq!(tracker.current().unwrap().state, ProgramStatusState::Done);

        // Soft reset DECSTR keeps records.
        let out = tracker.feed(b"\x1b[!p");
        assert!(!out.changed);
        assert_eq!(tracker.current().unwrap().state, ProgramStatusState::Done);

        // RIS full reset (even after aborted OSC) clears everything.
        let out = tracker.feed(b"\x1b]7501;state=working\x1bc");
        assert!(out.changed);
        assert!(tracker.current().is_none());
    }

    #[test]
    fn rejects_invalid_reports_duplicates_limits_and_disarms_bidi() {
        let mut tracker = ProgramStatusTracker::new();
        // Missing / unknown state.
        tracker.feed(b"\x1b]7501;app=codex\x1b\\");
        tracker.feed(b"\x1b]7501;state=bogus:app=codex\x1b\\");
        // Duplicate key cannot mask an earlier invalid value or limit violation.
        tracker.feed(b"\x1b]7501;state=bogus:state=working\x1b\\");
        tracker.feed(b"\x1b]7501;state=working:progress=999:progress=50\x1b\\");
        tracker.feed(b"\x1b]7501;state=working:id=a//b:id=ok\x1b\\");
        tracker.feed(b"\x1b]7501;state=working:id=a/b/c/d/e/f/g/h/i:id=ok\x1b\\");
        tracker.feed(b"\x1b]7501;AAAAAAAAAAAAAAAAA=1:state=working\x1b\\");
        // Exceeds MAX_SEQUENCE_BYTES (4096): 2 + 5 + 6 + 4082 + 2 = 4097 bytes.
        let oversized = format!("\x1b]7501;state=working:x={}\x1b\\", "a".repeat(4080));
        assert!(oversized.len() > MAX_SEQUENCE_BYTES);
        tracker.feed(oversized.as_bytes());
        // Control char or Unicode line separator in decoded msg is rejected whole.
        tracker
            .feed(format!("\x1b]7501;state=working:msg={}\x1b\\", b64("line1\nline2")).as_bytes());
        tracker.feed(
            format!(
                "\x1b]7501;state=working:msg={}\x1b\\",
                b64("line1\u{2028}line2")
            )
            .as_bytes(),
        );
        // Invalid base64 padding.
        tracker.feed(b"\x1b]7501;state=working:msg=A=\x1b\\");
        assert!(tracker.current().is_none());

        // Bidi-only text sanitizes to None rather than Some("").
        tracker.feed(
            format!(
                "\x1b]7501;state=working:app=codex:msg={}\x1b\\",
                b64("\u{202E}")
            )
            .as_bytes(),
        );
        assert_eq!(tracker.current().unwrap().msg, None);

        // Bidi override is stripped while valid text is kept.
        tracker.feed(
            format!(
                "\x1b]7501;state=working:app=codex:msg={}\x1b\\",
                b64("safe\u{202E}text")
            )
            .as_bytes(),
        );
        assert_eq!(tracker.current().unwrap().msg.as_deref(), Some("safetext"));
    }

    #[test]
    fn evicts_least_recently_updated_record_past_cap() {
        let mut tracker = ProgramStatusTracker::new();
        for i in 0..=MAX_RECORDS {
            tracker.feed(format!("\x1b]7501;state=idle:id=r{i}\x1b\\").as_bytes());
        }
        assert_eq!(tracker.records.len(), MAX_RECORDS);
        assert!(!tracker.records.contains_key("r0"));
        assert!(tracker.records.contains_key(&format!("r{MAX_RECORDS}")));
    }

    #[test]
    fn title_fallback_clears_on_global_clear_osc133_and_avoids_false_positives() {
        let mut tracker = ProgramStatusTracker::new();
        // Ordinary paths and editor titles must NOT false-positive.
        tracker.feed(b"\x1b]0;~/projects/claude\x07");
        assert!(tracker.current().is_none());
        tracker.feed(b"\x1b]0;vim codex.ts\x07");
        assert!(tracker.current().is_none());
        tracker.feed(b"\x1b]2;zsh (claude)\x1b\\");
        assert!(tracker.current().is_none());

        // Real Claude Code / Codex titles are recognized.
        let out = tracker.feed(b"\x1b]0;\xe2\x9c\xb3 Claude Code\x07");
        assert!(out.changed);
        assert_eq!(
            tracker.current().unwrap().app.as_deref(),
            Some("claude-code")
        );

        // Global OSC 7501 clear also clears fallback title_app.
        let out = tracker.feed(b"\x1b]7501;state=clear\x1b\\");
        assert!(out.changed);
        assert!(tracker.current().is_none());

        // Stale title_app is never merged onto an unrelated OSC 7501 record.
        tracker.feed(b"\x1b]0;\xe2\x9c\xb3 Claude Code\x07");
        tracker.feed(b"\x1b]7501;state=working\x1b\\");
        assert_eq!(tracker.current().unwrap().app, None);

        // OSC 133;A clears fallback title_app too.
        tracker.feed(b"\x1b]2;Codex\x1b\\");
        tracker.feed(b"\x1b]7501;state=clear\x1b\\");
        tracker.feed(b"\x1b]2;Codex\x1b\\");
        assert_eq!(tracker.current().unwrap().app.as_deref(), Some("codex"));
        let out = tracker.feed(b"\x1b]133;A\x07");
        assert!(out.changed);
        assert!(tracker.current().is_none());
    }
}
