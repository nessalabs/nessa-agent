//! pty-runtime's terminal client protocol, version 1
//! (`client/PROTOCOL.md` in nessalabs/pty-runtime), and the
//! projection-to-frame diff that makes it. Taken from that repository's
//! reference server (`client/server/src/wire.rs`, same revision as the
//! pinned crate) so the window paints exactly what its reference client
//! paints. The changes: `Row` and `Cursor` are `Clone` (a large frame is
//! cut into several), and `ClientMessage` is not needed here,
//! because a view's input arrives as product methods.
//!
//! The server renders, so everything here turns a `TerminalView` into
//! something a window can paint without knowing anything about terminals.
use pty_runtime::terminal::{
    MouseEncoding, MouseTracking, TerminalCell, TerminalColor, TerminalCursor, TerminalModes,
    TerminalPalette, TerminalStyle, TerminalView, Underline,
};
use serde::Serialize;
use std::collections::HashMap;

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub y: u16,
    /// `[text, width, style_id]` per cell.
    pub cells: Vec<(String, u8, u32)>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq, Hash, Clone)]
pub struct WireStyle {
    pub id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<[u8; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<[u8; 3]>,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub faint: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub inverse: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub invisible: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strikethrough: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub overline: bool,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub underline: &'static str,
}

/// Style identity without the allocated id, so interning can reuse entries.
#[derive(Debug, Default, PartialEq, Eq, Hash, Clone)]
struct StyleKey {
    fg: Option<[u8; 3]>,
    bg: Option<[u8; 3]>,
    bold: bool,
    italic: bool,
    faint: bool,
    inverse: bool,
    invisible: bool,
    strikethrough: bool,
    overline: bool,
    underline: &'static str,
}

impl From<&WireStyle> for StyleKey {
    fn from(style: &WireStyle) -> Self {
        Self {
            fg: style.fg,
            bg: style.bg,
            bold: style.bold,
            italic: style.italic,
            faint: style.faint,
            inverse: style.inverse,
            invisible: style.invisible,
            strikethrough: style.strikethrough,
            overline: style.overline,
            underline: style.underline,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMessage {
    Ready {
        v: u32,
        cols: u16,
        rows: u16,
        palette: WirePalette,
    },
    Styles {
        v: u32,
        styles: Vec<WireStyle>,
    },
    Modes {
        v: u32,
        alternate_screen: bool,
        bracketed_paste: bool,
        application_cursor: bool,
        /// Which events the program wants: none, press, press_release,
        /// button_motion or any_motion.
        mouse: &'static str,
        /// How it expects them encoded: legacy or sgr.
        mouse_encoding: &'static str,
    },
    Frame {
        v: u32,
        seq: u64,
        cursor: Cursor,
        rows: Vec<Row>,
        /// The current window, so a client knows how far back it can scroll
        /// without having to ask first.
        total: u64,
        scrollback: u64,
    },
    History {
        v: u32,
        /// Where the read actually began after clamping into the window.
        start: u64,
        rows: Vec<Row>,
        /// Rows the engine currently holds, and how many are scrollback. A
        /// client re-anchors from these when eviction moves the window.
        total: u64,
        scrollback: u64,
    },
    Exit {
        v: u32,
        status: Option<i32>,
    },
    Error {
        v: u32,
        message: String,
    },
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone)]
pub struct WirePalette {
    pub foreground: Option<[u8; 3]>,
    pub background: Option<[u8; 3]>,
    pub cursor: Option<[u8; 3]>,
    pub indexed: Vec<[u8; 3]>,
}

impl From<&TerminalPalette> for WirePalette {
    fn from(palette: &TerminalPalette) -> Self {
        Self {
            foreground: palette.foreground,
            background: palette.background,
            cursor: palette.cursor,
            indexed: palette.indexed.to_vec(),
        }
    }
}

/// Resolve a color to literal sRGB, or leave it to the client's default.
///
/// Palette entries are resolved here rather than in the browser so a client
/// never has to carry terminal color rules, and so an OSC palette override is
/// applied by whoever actually knows about it.
fn color(value: &TerminalColor, palette: &TerminalPalette) -> Option<[u8; 3]> {
    match value {
        TerminalColor::Default => None,
        TerminalColor::Palette(index) => Some(palette.indexed[*index as usize]),
        TerminalColor::Rgb(r, g, b) => Some([*r, *g, *b]),
    }
}

fn underline(value: Underline) -> &'static str {
    match value {
        Underline::None => "",
        Underline::Single => "single",
        Underline::Double => "double",
        Underline::Curly => "curly",
        Underline::Dotted => "dotted",
        Underline::Dashed => "dashed",
    }
}

/// Interns styles so a frame carries small integers rather than repeating a
/// full style on every cell. Terminals reuse a handful of styles across a whole
/// screen, so this is most of the wire saving.
///
/// The table is finite: a truecolor animation that keeps inventing RGB pairs
/// cannot grow it without bound. At capacity the table resets, identifiers
/// restart at 1, and the next `ready` / `styles` exchange replaces the client's
/// map before those ids are reused.
const STYLE_CAP: usize = 512;

#[derive(Default)]
pub struct StyleTable {
    ids: HashMap<StyleKey, u32>,
    pending: Vec<WireStyle>,
    /// Set when ids were discarded mid-encode; the caller must rebuild the frame.
    restarted: bool,
    /// After one reset in an encode pass, further new styles map to default (0).
    capped: bool,
}

impl StyleTable {
    pub fn begin_encode(&mut self) {
        self.restarted = false;
        self.capped = false;
    }

    pub fn take_restarted(&mut self) -> bool {
        std::mem::take(&mut self.restarted)
    }

    pub fn intern(&mut self, style: &TerminalStyle, palette: &TerminalPalette) -> u32 {
        let wire = WireStyle {
            id: 0,
            fg: color(&style.foreground, palette),
            bg: color(&style.background, palette),
            bold: style.bold,
            italic: style.italic,
            faint: style.faint,
            inverse: style.inverse,
            invisible: style.invisible,
            strikethrough: style.strikethrough,
            overline: style.overline,
            underline: underline(style.underline),
        };
        if StyleKey::from(&wire) == StyleKey::default() {
            return 0;
        }
        let key = StyleKey::from(&wire);
        if let Some(id) = self.ids.get(&key) {
            return *id;
        }
        if self.ids.len() >= STYLE_CAP {
            if self.capped {
                // One screen can still invent more distinct colours than the
                // budget; refuse further growth rather than looping resets.
                return 0;
            }
            self.ids.clear();
            self.pending.clear();
            self.restarted = true;
            self.capped = true;
        }
        // Zero is reserved for the default style, so identifiers start at one.
        let id = self.ids.len() as u32 + 1;
        let mut wire = wire;
        wire.id = id;
        self.ids.insert(key, id);
        self.pending.push(wire);
        id
    }

    /// Styles first seen since the previous call, to send before the frame
    /// that references them.
    pub fn take_pending(&mut self) -> Vec<WireStyle> {
        std::mem::take(&mut self.pending)
    }

    /// Drop every interned style. Required when the client is told to forget
    /// its table (a new `ready`), or previously allocated ids would be reused
    /// without retransmission.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.pending.clear();
        self.restarted = false;
        self.capped = false;
    }
}

fn encode_row(cells: &[TerminalCell], styles: &mut StyleTable, palette: &TerminalPalette) -> Row {
    Row {
        y: 0,
        cells: cells
            .iter()
            .map(|cell| {
                (
                    cell.text.clone(),
                    cell.width,
                    styles.intern(&cell.style, palette),
                )
            })
            .collect(),
    }
}

/// Encode retained rows for the wire, reusing the row encoding a frame uses.
/// The bool is true when the shared style table had to reset; the caller must
/// send `ready` before `styles` so the client drops stale identifiers.
pub fn encode_history(
    history: &pty_runtime::terminal::TerminalHistory,
    styles: &mut StyleTable,
    palette: &TerminalPalette,
) -> (Vec<Row>, Vec<WireStyle>, bool) {
    let cols = usize::from(history.cols);
    let mut reset = false;
    loop {
        styles.begin_encode();
        let mut rows = Vec::new();
        if cols > 0 {
            for (index, chunk) in history.cells.chunks(cols).enumerate() {
                let mut row = encode_row(chunk, styles, palette);
                // Rows are numbered absolutely so a client can place them without
                // tracking how many it has already received.
                row.y = u16::try_from(history.start + index as u64).unwrap_or(u16::MAX);
                rows.push(row);
            }
        }
        if !styles.take_restarted() {
            return (rows, styles.take_pending(), reset);
        }
        reset = true;
        styles.clear();
    }
}

/// What the client has already been shown, so only differences are sent.
#[derive(Default)]
pub struct Sent {
    rows: Vec<Vec<TerminalCell>>,
    size: Option<(u16, u16)>,
    palette: Option<WirePalette>,
    modes: Option<TerminalModes>,
    cursor: Option<TerminalCursor>,
    totals: Option<(u64, u64)>,
    seq: u64,
}

impl Sent {
    /// Messages that bring the client up to date, in the order they must be
    /// sent: styles before the frame that uses them.
    pub fn diff(
        &mut self,
        view: &TerminalView,
        styles: &mut StyleTable,
        total: u64,
        scrollback: u64,
    ) -> Vec<ServerMessage> {
        let mut messages = Vec::new();
        let cols = usize::from(view.size.cols());
        let rows = usize::from(view.size.rows());

        // `ready` carries the geometry as well as the palette, so it has to be
        // re-sent when either changes. A resize that only moved the dimensions
        // would otherwise leave the client rendering at the previous width,
        // which is what a shrink followed by a grow used to look like.
        let palette = WirePalette::from(&view.palette);
        let size = (view.size.cols(), view.size.rows());
        let palette_changed = self.palette.as_ref() != Some(&palette);
        let size_changed = self.size != Some(size);
        if palette_changed || size_changed {
            messages.push(ServerMessage::Ready {
                v: VERSION,
                cols: size.0,
                rows: size.1,
                palette: palette.clone(),
            });
            self.palette = Some(palette);
            self.size = Some(size);
            // Geometry or palette changes force a full frame rewrite. Drop the
            // style table too: the client clears its map on every `ready`, so
            // ids must be retransmitted (and a long session cannot keep an
            // unbounded interned set across resizes either).
            self.rows.clear();
            styles.clear();
        }

        if self.modes != Some(view.modes) {
            messages.push(ServerMessage::Modes {
                v: VERSION,
                alternate_screen: view.modes.alternate_screen,
                bracketed_paste: view.modes.bracketed_paste,
                application_cursor: view.modes.application_cursor,
                mouse: match view.modes.mouse {
                    MouseTracking::None => "none",
                    MouseTracking::Press => "press",
                    MouseTracking::PressRelease => "press_release",
                    MouseTracking::ButtonMotion => "button_motion",
                    MouseTracking::AnyMotion => "any_motion",
                },
                mouse_encoding: match view.modes.mouse_encoding {
                    MouseEncoding::Legacy => "legacy",
                    MouseEncoding::Sgr => "sgr",
                },
            });
            self.modes = Some(view.modes);
        }

        // A resize changes what every row means, so nothing retained about the
        // old geometry can be compared against the new one.
        if self.rows.len() != rows || self.rows.first().is_some_and(|r| r.len() != cols) {
            self.rows = vec![Vec::new(); rows];
        }

        let mut changed;
        let mut style_reset = false;
        loop {
            styles.begin_encode();
            changed = Vec::new();
            for y in 0..rows {
                let start = y * cols;
                let Some(row) = view.cells.get(start..start + cols) else {
                    break;
                };
                if self.rows[y] == row {
                    continue;
                }
                let mut encoded = encode_row(row, styles, &view.palette);
                encoded.y = y as u16;
                changed.push(encoded);
                self.rows[y] = row.to_vec();
            }
            if !styles.take_restarted() {
                break;
            }
            // Ids were recycled mid-frame; rebuild from a clean table so no
            // cell keeps a pre-reset identifier.
            style_reset = true;
            styles.clear();
            self.rows = vec![Vec::new(); rows];
        }

        if style_reset {
            // Same geometry/palette, but the client must drop stale ids before
            // the new `styles` message reuses them.
            if !messages
                .iter()
                .any(|message| matches!(message, ServerMessage::Ready { .. }))
            {
                messages.push(ServerMessage::Ready {
                    v: VERSION,
                    cols: size.0,
                    rows: size.1,
                    palette: WirePalette::from(&view.palette),
                });
            }
        }

        let pending = styles.take_pending();
        if !pending.is_empty() {
            messages.push(ServerMessage::Styles {
                v: VERSION,
                styles: pending,
            });
        }

        // The cursor moves without any cell changing: backspace at a prompt,
        // arrow keys, a program repositioning. Diffing rows alone would leave
        // the caret stranded where it last happened to be drawn.
        let cursor_moved = self.cursor != Some(view.cursor);
        self.cursor = Some(view.cursor);

        // History can grow or evict while the visible cells and the cursor
        // stay exactly as they were -- blank output at a pinned prompt does
        // both. The client clamps its scroll requests against these counts, so
        // leaving them unsent strands it at a position it can no longer leave.
        let totals_moved = self.totals != Some((total, scrollback));
        self.totals = Some((total, scrollback));

        if !changed.is_empty() || cursor_moved || totals_moved || self.seq == 0 {
            self.seq += 1;
            messages.push(ServerMessage::Frame {
                v: VERSION,
                seq: self.seq,
                cursor: Cursor {
                    col: view.cursor.col,
                    row: view.cursor.row,
                    visible: view.cursor.visible,
                },
                rows: changed,
                total,
                scrollback,
            });
        }

        messages
    }
}
