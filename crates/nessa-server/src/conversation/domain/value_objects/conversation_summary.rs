/// What a conversation is called in a list of conversations.
///
/// Derived from the first message a person sends, and then kept: a later
/// message does not rename the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationTitle(String);
impl ConversationTitle {
    /// Longest title, in characters. The length is the one the panel used
    /// for tab titles before the gateway kept them; the rule itself is this
    /// type's own ([`Self::for_message`]): the first line of the message read
    /// as plain text, else the first linked file's name, else "Image".
    pub const MAX_CHARS: usize = 48;
    /// What a message of images alone is called, when it names no file either.
    const IMAGE: &'static str = "Image";

    /// Accept a title read back from storage. It must be exactly what
    /// [`Self::for_message`] could have produced: one line, no surrounding or
    /// doubled whitespace, and no longer than [`Self::MAX_CHARS`].
    pub fn new(value: &str) -> Result<Self, &'static str> {
        if value.is_empty() || one_line(value) != value || value.chars().count() > Self::MAX_CHARS {
            return Err("invalid conversation title");
        }
        Ok(Self(value.to_owned()))
    }
    /// The title a conversation takes from its first message: the first line
    /// of its text, read as plain text the way a preview is — a title is shown
    /// in the same list, where `**3-day**` would be two pairs of asterisks;
    /// with no text, the name of the first file it points at; and with
    /// neither, [`Self::IMAGE`], since a message is never empty.
    ///
    /// `first_file_name` is the file's own name, as the message's linked file
    /// reports it — this does not take a path apart.
    ///
    /// Read from the message's [`opening`]: lines with nothing visible once
    /// their marks are dropped — blank, control characters alone, a bare
    /// quote marker — are passed over however many there are, and a first
    /// line longer than [`READ_BYTES`] is read only that far
    /// (`a_title_and_a_preview_read_the_same_bounded_opening`).
    pub fn for_message(text: &str, first_file_name: Option<&str>) -> Self {
        let plain = opening(text);
        let first_line = plain.lines().map(one_line).find(|line| !line.is_empty());
        let title = first_line
            .into_iter()
            .chain(first_file_name.map(one_line))
            .find(|line| !line.is_empty())
            .map(|line| match line.char_indices().nth(Self::MAX_CHARS) {
                Some((end, _)) => line[..end].trim_end().to_owned(),
                None => line,
            });
        title.map_or_else(|| Self(Self::IMAGE.to_owned()), Self)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The last thing said in a conversation, as one line of plain text.
///
/// A reply is Markdown and a list row is not, so the marks that only mean
/// something once rendered are dropped before the text is folded onto one
/// line: a heading's `#`, a list's bullet, emphasis around a word, the
/// backticks around code, the address behind a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationPreview(String);
impl ConversationPreview {
    /// Longest preview, in UTF-8 bytes. Cut on a character boundary.
    pub const MAX_BYTES: usize = 512;
    /// What a message with no text says in a list.
    const ATTACHMENT: &'static str = "Attachment";

    /// Accept a preview read back from storage: one line, no surrounding or
    /// doubled whitespace, and at most [`Self::MAX_BYTES`].
    pub fn new(value: &str) -> Result<Self, &'static str> {
        if value.is_empty() || one_line(value) != value || value.len() > Self::MAX_BYTES {
            return Err("invalid conversation preview");
        }
        Ok(Self(value.to_owned()))
    }
    /// The preview of `text`, or `None` when nothing is left of it once its
    /// marks and whitespace are gone. Read from the message's [`opening`], as
    /// a title is, so leading lines with nothing visible never push what
    /// follows them out of the preview
    /// (`a_title_and_a_preview_read_the_same_bounded_opening`).
    pub fn from_text(text: &str) -> Option<Self> {
        let plain = opening(text);
        let line = one_line(&plain);
        let preview = clip(&line, Self::MAX_BYTES).trim_end();
        (!preview.is_empty()).then(|| Self(preview.to_owned()))
    }
    /// The preview of a message that carried files and no text.
    pub fn attachment() -> Self {
        Self(Self::ATTACHMENT.to_owned())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a list of conversations shows about one of them, besides who owns it.
///
/// A projection of what was said, kept apart from the ownership record, which
/// is written once and never changes. Each change produces a new summary.
///
/// One field is not a projection: whether somebody archived the conversation.
/// That is a person's decision, kept here because it is about how the
/// conversation is listed, and every change below says what becomes of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationSummary {
    title: Option<ConversationTitle>,
    preview: Option<ConversationPreview>,
    updated_at_ms: u64,
    archived: bool,
}
/// The latest time a conversation's times may hold, in Unix milliseconds:
/// the largest integer a JSON number carries exactly, which is what a list
/// promises its readers. A time past it, read back from a damaged file, would
/// make every reader refuse the whole list for one row
/// (`the_list_schema_states_the_bounds_the_summary_rules_keep`).
pub const LATEST_TIME_MS: u64 = 9_007_199_254_740_991;

impl ConversationSummary {
    /// A summary read back from storage.
    ///
    /// # Errors
    /// A time past [`LATEST_TIME_MS`], which no summary written here holds
    /// (`a_summary_dated_past_the_latest_time_is_unreadable`).
    pub fn new(
        title: Option<ConversationTitle>,
        preview: Option<ConversationPreview>,
        updated_at_ms: u64,
        archived: bool,
    ) -> Result<Self, &'static str> {
        if updated_at_ms > LATEST_TIME_MS {
            return Err("conversation summary time out of range");
        }
        Ok(Self {
            title,
            preview,
            updated_at_ms,
            archived,
        })
    }
    /// After a person's message was accepted. The title is taken only if the
    /// conversation has none yet; the preview is the message, or
    /// [`ConversationPreview::attachment`] for one with no text.
    ///
    /// The time never moves backwards: a clock stepped back does not make a
    /// conversation older than something already said in it
    /// (`a_summary_keeps_its_first_title_and_follows_what_was_said_last`).
    ///
    /// A message unarchives the conversation: one somebody is talking in is
    /// not archived (`a_new_message_unarchives_and_a_reply_does_not`).
    pub fn after_message(
        previous: Option<&Self>,
        text: &str,
        first_file_name: Option<&str>,
        at_ms: u64,
    ) -> Self {
        Self {
            title: Some(
                previous
                    .and_then(|summary| summary.title.clone())
                    .unwrap_or_else(|| ConversationTitle::for_message(text, first_file_name)),
            ),
            preview: Some(
                ConversationPreview::from_text(text)
                    .unwrap_or_else(ConversationPreview::attachment),
            ),
            updated_at_ms: later(previous, at_ms),
            archived: false,
        }
    }
    /// After the agent replied, or `None` when the reply said nothing a
    /// preview could show. A reply never titles a conversation; the same test
    /// as [`Self::after_message`] holds both to that. Nor does it unarchive
    /// one: a turn finishing after its conversation was archived is not
    /// somebody talking in it.
    pub fn after_reply(previous: Option<&Self>, reply: &str, at_ms: u64) -> Option<Self> {
        Some(Self {
            title: previous.and_then(|summary| summary.title.clone()),
            preview: Some(ConversationPreview::from_text(reply)?),
            updated_at_ms: later(previous, at_ms),
            archived: previous.is_some_and(|summary| summary.archived),
        })
    }
    /// After somebody archived the conversation, or unarchived it. Nothing
    /// else changes, and nothing was said, so the time does not move.
    ///
    /// Only of a summary: a conversation nothing was said in has none, is not
    /// listed, and so has no listing to archive — a summary exists exactly
    /// when something was said, which is the one rule a list is drawn by
    /// (`archiving_a_conversation_nothing_was_said_in_changes_nothing`).
    pub fn after_archiving(&self, archived: bool) -> Self {
        Self {
            archived,
            ..self.clone()
        }
    }
    pub fn title(&self) -> Option<&ConversationTitle> {
        self.title.as_ref()
    }
    pub fn preview(&self) -> Option<&ConversationPreview> {
        self.preview.as_ref()
    }
    pub fn updated_at_ms(&self) -> u64 {
        self.updated_at_ms
    }
    /// Whether somebody archived the conversation and nothing has been said
    /// in it since.
    pub fn archived(&self) -> bool {
        self.archived
    }
}

/// When something said at `at_ms` is recorded as said: never before what was
/// said already, and never past [`LATEST_TIME_MS`], so no summary built here
/// holds a time a list could not carry.
fn later(previous: Option<&ConversationSummary>, at_ms: u64) -> u64 {
    previous
        .map_or(at_ms, |summary| summary.updated_at_ms.max(at_ms))
        .min(LATEST_TIME_MS)
}

/// How much of a message is read to title it or preview it, from its
/// [`opening`]. Both are its beginning, so nothing further can reach them
/// except through marks dropped before it, and the bound keeps the work per
/// message fixed.
const READ_BYTES: usize = 8192;

/// The plain text of `text`'s opening: [`READ_BYTES`] of it, from where the
/// first line with anything visible once its marks are dropped begins to say
/// it — after its heading, quote, or list markers.
///
/// The one rule a title and a preview are both read by. Finding that line is
/// linear in the text and bounded: a line with nothing but whitespace and
/// control characters once its markers are gone is passed over at a glance,
/// and only a line with something left is read as plain text, at most
/// [`READ_BYTES`] of it — and at most [`READ_BYTES`] across all such lines
/// before the opening is taken to start at the next one, visible or not.
fn opening(text: &str) -> String {
    let mut read = 0;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let said = without_line_markers(content);
        if said.chars().any(|c| !(c.is_whitespace() || c.is_control())) {
            let start = at + (content.len() - said.len());
            if read >= READ_BYTES {
                return plain_text(clip(&text[start..], READ_BYTES));
            }
            let probe = clip(said, READ_BYTES);
            read += probe.len();
            if !one_line(&plain_text(probe)).is_empty() {
                return plain_text(clip(&text[start..], READ_BYTES));
            }
        }
        at += line.len();
    }
    String::new()
}

/// The longest prefix of `value` of at most `bytes`, ending on a character.
fn clip(value: &str, bytes: usize) -> &str {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Every run of whitespace or control characters becomes one space, and none
/// is left at either end, so what remains is one line whatever it held.
fn one_line(value: &str) -> String {
    value
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` with the Markdown marks a reader would not see dropped, line by
/// line. Nothing here spans lines, so neither does anything it drops.
fn plain_text(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    for line in text.lines() {
        let chars: Vec<char> = without_line_markers(line).chars().collect();
        plain_inline(&chars, &mut plain);
        plain.push('\n');
    }
    plain
}

/// A line without its heading, quote, or list markers, after at most three
/// spaces of indentation. Markers nest (`> - item`), so they are taken until
/// none is left.
fn without_line_markers(mut line: &str) -> &str {
    loop {
        let indented = line.trim_start_matches(' ');
        if line.len() - indented.len() > 3 {
            return line;
        }
        match after_line_marker(indented) {
            Some(rest) => line = rest,
            None => return line,
        }
    }
}

fn after_line_marker(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes) {
        return line[hashes..].strip_prefix(' ');
    }
    for marker in ["> ", "- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=9).contains(&digits) {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// Append `chars` to `plain` with links reduced to their text and emphasis,
/// strike-through and code spans reduced to what they enclose.
fn plain_inline(chars: &[char], plain: &mut String) {
    let mut at = 0;
    while at < chars.len() {
        let current = chars[at];
        let bracket = if current == '!' { at + 1 } else { at };
        if chars.get(bracket) == Some(&'[') {
            if let Some((text_end, after)) = link(chars, bracket) {
                plain_inline(&chars[bracket + 1..text_end], plain);
                at = after;
                continue;
            }
        }
        if matches!(current, '*' | '_' | '~' | '`') {
            let run = run_length(chars, at);
            if let Some(close) = closing_delimiter(chars, at, run) {
                let inner = &chars[at + run..close];
                if current == '`' {
                    plain.extend(inner);
                } else {
                    plain_inline(inner, plain);
                }
                at = close + run;
            } else {
                plain.extend(&chars[at..at + run]);
                at += run;
            }
            continue;
        }
        plain.push(current);
        at += 1;
    }
}

/// For `[text](address)` opening at `open`: where its text ends, and where
/// the whole link does.
fn link(chars: &[char], open: usize) -> Option<(usize, usize)> {
    let text_end = open + 1 + chars[open + 1..].iter().position(|c| *c == ']')?;
    if chars.get(text_end + 1) != Some(&'(') {
        return None;
    }
    let address_end = text_end + 2 + chars[text_end + 2..].iter().position(|c| *c == ')')?;
    Some((text_end, address_end + 1))
}

fn run_length(chars: &[char], at: usize) -> usize {
    chars[at..].iter().take_while(|c| **c == chars[at]).count()
}

/// Where the delimiter run of `run` characters opening at `open` closes, if it
/// is one. It opens only at a word boundary and before something other than
/// whitespace, and closes on a run of the same length after something other
/// than whitespace and before a character that is not part of a word — so the
/// underscores in `snake_case_dir` stay where they are.
fn closing_delimiter(chars: &[char], open: usize, run: usize) -> Option<usize> {
    let delimiter = chars[open];
    let is_mark = match delimiter {
        '*' | '_' => run <= 2,
        '~' => run == 2,
        _ => true,
    };
    let after_word = open
        .checked_sub(1)
        .is_some_and(|before| chars[before].is_alphanumeric());
    if !is_mark || after_word || chars.get(open + run).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut at = open + run;
    while at < chars.len() {
        if chars[at] != delimiter {
            at += 1;
            continue;
        }
        let length = run_length(chars, at);
        if length == run
            && !chars[at - 1].is_whitespace()
            && !chars
                .get(at + length)
                .is_some_and(|next| next.is_alphanumeric())
        {
            return Some(at);
        }
        at += length;
    }
    None
}
