/// A coding agent Nessa knows by name.
///
/// Only agents Nessa has an adapter for are listed. An agent absent from here
/// is not one the server has an opinion about — it is one Nessa cannot drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentId {
    Claude,
}

impl AgentId {
    /// Every agent the server will report on, in listing order.
    pub const ALL: &'static [AgentId] = &[AgentId::Claude];
}
