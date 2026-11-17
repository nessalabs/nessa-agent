//! Which of the gateway's paired computers new work goes to first, and which
//! it never goes to. Every connected computer is in the pool unless left out;
//! the order says which one a new thread tries first, second, and so on.
//!
//! A channel either inherits the Nessa default (no override) or replaces it
//! whole with its own preference: a channel's order and left-out list are
//! read together, never merged with the default's.
use super::settings_scope::SettingsScope;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;

/// The most computers one preference's order, or its left-out list, names.
/// A computer named in neither is still in the pool, after the ordered ones.
pub const MAX_POOL_ENTRIES: usize = 64;

/// Why a preference was refused before anything was saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolRefusal {
    /// More than [`MAX_POOL_ENTRIES`] in the order or in the left-out list.
    TooMany,
    /// One computer named twice, or both ordered and left out.
    Repeated,
}

/// Where new work goes first, and where it never goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolPreference {
    order: Vec<PeerEnvironment>,
    excluded: Vec<PeerEnvironment>,
}

impl PoolPreference {
    /// `order`, first choice first, and the computers left out.
    ///
    /// # Errors
    /// [`PoolRefusal`] when either list is too long, or a computer is named
    /// twice, in one list or both.
    pub fn new(
        order: Vec<PeerEnvironment>,
        excluded: Vec<PeerEnvironment>,
    ) -> Result<Self, PoolRefusal> {
        if order.len() > MAX_POOL_ENTRIES || excluded.len() > MAX_POOL_ENTRIES {
            return Err(PoolRefusal::TooMany);
        }
        let mut seen = std::collections::BTreeSet::new();
        if !order.iter().chain(&excluded).all(|peer| seen.insert(*peer)) {
            return Err(PoolRefusal::Repeated);
        }
        Ok(Self { order, excluded })
    }

    /// The ordered computers, first choice first.
    pub fn order(&self) -> &[PeerEnvironment] {
        &self.order
    }

    /// The computers new work never goes to.
    pub fn excluded(&self) -> &[PeerEnvironment] {
        &self.excluded
    }

    /// Whether new work may go to `peer`.
    pub fn includes(&self, peer: &PeerEnvironment) -> bool {
        !self.excluded.contains(peer)
    }

    /// Where `peer` stands: its place in the order, or after every ordered
    /// computer when it is not named there. Lower goes first.
    pub fn rank(&self, peer: &PeerEnvironment) -> usize {
        self.order
            .iter()
            .position(|ordered| ordered == peer)
            .unwrap_or(self.order.len())
    }
}

/// The Nessa default, a channel's override of it, and when a computer counts
/// as too busy for new work, read together from one configuration document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolSettings {
    scope: SettingsScope,
    default: PoolPreference,
    channel: Option<PoolPreference>,
    busy_cpu_permille: u32,
    busy_memory_permille: u32,
}

impl PoolSettings {
    /// What one read of `scope` found.
    pub fn new(
        scope: SettingsScope,
        default: PoolPreference,
        channel: Option<PoolPreference>,
        busy_cpu_permille: u32,
        busy_memory_permille: u32,
    ) -> Self {
        Self {
            // A global read holds no channel override.
            channel: channel.filter(|_| scope.channel().is_some()),
            scope,
            default,
            busy_cpu_permille,
            busy_memory_permille,
        }
    }

    /// What was read.
    pub fn scope(&self) -> &SettingsScope {
        &self.scope
    }

    /// The Nessa default.
    pub fn default_preference(&self) -> &PoolPreference {
        &self.default
    }

    /// The channel's own preference; absent when it inherits the default.
    pub fn channel_override(&self) -> Option<&PoolPreference> {
        self.channel.as_ref()
    }

    /// The preference new work in this scope follows: the channel's, or the
    /// default when it inherits.
    pub fn effective(&self) -> &PoolPreference {
        crate::settings::resolve_inherited(&self.default, [self.channel.as_ref()])
    }

    /// The processor and memory use, in whole percent, at which a computer
    /// is busy.
    pub fn busy_percents(&self) -> (u32, u32) {
        (self.busy_cpu_permille / 10, self.busy_memory_permille / 10)
    }

    /// Whether a computer reporting `cpu_permille` processor use and
    /// `memory_permille` memory use is too busy to be given new work while
    /// another is not. A computer that never reported is not busy.
    pub fn busy(&self, cpu_permille: Option<u32>, memory_permille: Option<u32>) -> bool {
        cpu_permille.is_some_and(|used| used >= self.busy_cpu_permille)
            || memory_permille.is_some_and(|used| used >= self.busy_memory_permille)
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/pool_preference.rs"]
mod tests;
