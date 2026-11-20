//! How an agent talks to the person: Nessa's plain default, or a person's own
//! text, chosen for each role at the Nessa level or for one channel.
//! Inheriting is the absence of a choice; [`ReplyStyle::Off`] is a choice.
use super::settings_scope::SettingsScope;

/// The most bytes of a person's own style text. Product protocol generation
/// checks its published bound against this one.
pub const MAX_REPLY_STYLE_BYTES: usize = 4096;

/// Which agents a style is for. A channel's coordinator and the threads it
/// starts are told different things, so each has its own setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StyleRole {
    /// The one agent in a channel that runs things.
    Coordinator,
    /// Every other conversation, in a channel or not.
    Thread,
}
impl StyleRole {
    pub const ALL: [Self; 2] = [Self::Coordinator, Self::Thread];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Thread => "thread",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == value)
    }
}

/// Rules every agent follows unless the person chose otherwise.
const PLAIN: &str = "How to write to the person:\n\
- Lead with the answer or the ask. Explain after.\n\
- Use short sentences, one idea in each.\n\
- Use plain words. If there is a simpler word, use it.\n\
- Keep paragraphs to three sentences or fewer.\n\
- Use a list only when the things in it really are a list.\n\
- Give a small, real example when an idea is hard to picture.\n\
- Leave out what is not needed: no recap and no filler.";
const COORDINATOR: &str = "\n- Be very short: status, what is next, and links to threads. Pass detailed questions to the thread that owns them, on the person's behalf, and say where you asked.";
const THREAD: &str = "\n- Be specific: name the files, commands and results, still in plain words.";

/// A person's own style text: trimmed, non-empty and at most
/// [`MAX_REPLY_STYLE_BYTES`]. Made only through [`StyleText::new`], and never
/// changed after, so every kept or told text holds to that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleText(String);
impl StyleText {
    /// `text` trimmed, refused when empty or too long.
    pub fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        (!text.is_empty() && text.len() <= MAX_REPLY_STYLE_BYTES).then(|| Self(text.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One style choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyStyle {
    /// Nessa's own plain style for the role.
    Plain,
    /// No style instructions at all: the agent talks as its harness does.
    Off,
    /// The person's own words.
    Custom(StyleText),
}
impl ReplyStyle {
    /// The person's own text, refused when empty or too long.
    pub fn custom(text: &str) -> Option<Self> {
        StyleText::new(text).map(Self::Custom)
    }
    /// What the agent is told, for `role`; `None` for [`Self::Off`].
    pub fn instructions(&self, role: StyleRole) -> Option<String> {
        match self {
            Self::Off => None,
            Self::Plain => Some(format!(
                "{PLAIN}{}",
                match role {
                    StyleRole::Coordinator => COORDINATOR,
                    StyleRole::Thread => THREAD,
                }
            )),
            Self::Custom(text) => Some(text.as_str().to_owned()),
        }
    }
}

/// One choice for each role; `None` inherits from the level above.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleStyles {
    pub coordinator: Option<ReplyStyle>,
    pub thread: Option<ReplyStyle>,
}
impl RoleStyles {
    pub fn get(&self, role: StyleRole) -> Option<&ReplyStyle> {
        match role {
            StyleRole::Coordinator => self.coordinator.as_ref(),
            StyleRole::Thread => self.thread.as_ref(),
        }
    }
    /// These choices with `role`'s replaced, or cleared by `None`.
    #[must_use]
    pub fn with(&self, role: StyleRole, choice: Option<ReplyStyle>) -> Self {
        let mut next = self.clone();
        match role {
            StyleRole::Coordinator => next.coordinator = choice,
            StyleRole::Thread => next.thread = choice,
        }
        next
    }
}

/// The Nessa-level and channel choices observed together for one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyStyleSettings {
    scope: SettingsScope,
    nessa: RoleStyles,
    channel: RoleStyles,
}
impl ReplyStyleSettings {
    /// `channel` is ignored for [`SettingsScope::Global`]: a global read has
    /// no channel choices.
    pub fn new(scope: SettingsScope, nessa: RoleStyles, channel: RoleStyles) -> Self {
        let channel = match scope {
            SettingsScope::Global => RoleStyles::default(),
            SettingsScope::Channel(_) => channel,
        };
        Self {
            scope,
            nessa,
            channel,
        }
    }
    pub fn scope(&self) -> &SettingsScope {
        &self.scope
    }
    /// The Nessa-level choices; `None` uses [`ReplyStyle::Plain`].
    pub fn nessa(&self) -> &RoleStyles {
        &self.nessa
    }
    /// This channel's own choices; `None` inherits the Nessa level.
    pub fn channel(&self) -> &RoleStyles {
        &self.channel
    }
    /// The style `role` gets here: the channel's choice, else the Nessa
    /// level's, else Nessa's plain style.
    pub fn effective(&self, role: StyleRole) -> ReplyStyle {
        crate::settings::resolve_inherited(
            ReplyStyle::Plain,
            [
                self.nessa.get(role).cloned(),
                self.channel.get(role).cloned(),
            ],
        )
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/reply_style.rs"]
mod tests;
