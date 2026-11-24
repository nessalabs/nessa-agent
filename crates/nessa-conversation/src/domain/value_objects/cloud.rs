//! The clouds a thread's VM can be made on, named the same way in a
//! channel's places and in the workspaces that make the VMs.

/// The clouds a thread's VM can run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cloud {
    Boat,
    Agent37,
}
impl Cloud {
    /// Every cloud this gateway knows, in the order it offers them.
    pub const ALL: [Self; 2] = [Self::Boat, Self::Agent37];
    /// The cloud `word` names, as [`Self::as_str`] spells it.
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cloud| cloud.as_str() == word)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boat => "boat",
            Self::Agent37 => "agent37",
        }
    }
}
