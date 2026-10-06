//! Tier-3 operational limits: counts and the cold-read budget a deployment
//! may set, validated once into a value nothing later mutates.
//!
//! The numbers a fresh gateway uses live only in [`OperationalLimits::default`].
//! A configuration file replaces a field or it does not; it never invents a
//! second default. Fixed limits (a socket's record slot, the request frame)
//! stay with their owners and are not fields here.
use crate::conversation::infrastructure::READ_WORK_BUDGET;
use std::time::{Duration, Instant};

/// The most permits one semaphore is built with. Above this, `Semaphore::new`
/// panics; the value object refuses the count first.
const MAX_ADMISSION: u64 = 65_536;

/// The numbers a configuration file stated, before they are checked.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConfiguredLimits {
    pub requests: u64,
    pub controls: u64,
    pub record_reads: u64,
    pub upload_begins: u64,
    pub deletions: u64,
    pub ordinary_slots: u64,
    pub control_slots: u64,
    pub app_calls_per_socket: u64,
    pub read_work_budget: Duration,
}

/// How many of each admission a gateway allows, and how long one cold read
/// may look before it answers that the source is still preparing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationalLimits {
    requests: usize,
    controls: usize,
    record_reads: usize,
    upload_begins: usize,
    deletions: usize,
    ordinary_slots: usize,
    control_slots: usize,
    app_calls_per_socket: usize,
    read_work_budget: Duration,
}

/// Which configured limit could not be used.
///
/// A variant rather than a parsed sentence, so a caller decides on the field
/// rather than on the wording of a message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidOperationalLimits {
    Requests,
    Controls,
    RecordReads,
    UploadBegins,
    Deletions,
    OrdinarySlots,
    ControlSlots,
    /// App calls must leave one slot another mount can take.
    AppCalls,
    ReadWorkBudget,
}

impl InvalidOperationalLimits {
    /// The rejected field's name, for a message.
    pub fn field(self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::Controls => "controls",
            Self::RecordReads => "record_reads",
            Self::UploadBegins => "upload_begins",
            Self::Deletions => "deletions",
            Self::OrdinarySlots => "ordinary_slots",
            Self::ControlSlots => "control_slots",
            Self::AppCalls => "app_calls_per_socket",
            Self::ReadWorkBudget => "read_work_budget",
        }
    }
}

impl std::fmt::Display for InvalidOperationalLimits {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppCalls => write!(
                formatter,
                "app_calls_per_socket must be from 2 to {MAX_ADMISSION}"
            ),
            Self::ReadWorkBudget => write!(
                formatter,
                "read_work_budget must be positive and representable as a deadline"
            ),
            other => write!(
                formatter,
                "{} must be from 1 to {MAX_ADMISSION}",
                other.field()
            ),
        }
    }
}

impl std::error::Error for InvalidOperationalLimits {}

impl Default for OperationalLimits {
    fn default() -> Self {
        Self {
            requests: 128,
            controls: 32,
            record_reads: 4,
            upload_begins: 16,
            deletions: 8,
            ordinary_slots: 16,
            control_slots: 4,
            app_calls_per_socket: 4,
            read_work_budget: READ_WORK_BUDGET,
        }
    }
}

impl OperationalLimits {
    /// Check `input`. A count outside 1..=65536, fewer than two app calls, or
    /// a budget that cannot be a deadline is refused. Nothing here is mutated
    /// afterward.
    pub(crate) fn configured(input: ConfiguredLimits) -> Result<Self, InvalidOperationalLimits> {
        Ok(Self {
            requests: admit(input.requests, InvalidOperationalLimits::Requests)?,
            controls: admit(input.controls, InvalidOperationalLimits::Controls)?,
            record_reads: admit(input.record_reads, InvalidOperationalLimits::RecordReads)?,
            upload_begins: admit(input.upload_begins, InvalidOperationalLimits::UploadBegins)?,
            deletions: admit(input.deletions, InvalidOperationalLimits::Deletions)?,
            ordinary_slots: admit(
                input.ordinary_slots,
                InvalidOperationalLimits::OrdinarySlots,
            )?,
            control_slots: admit(input.control_slots, InvalidOperationalLimits::ControlSlots)?,
            app_calls_per_socket: app_calls(input.app_calls_per_socket)?,
            read_work_budget: budget(input.read_work_budget)?,
        })
    }

    pub(crate) fn requests(self) -> usize {
        self.requests
    }
    pub(crate) fn controls(self) -> usize {
        self.controls
    }
    pub(crate) fn record_reads(self) -> usize {
        self.record_reads
    }
    pub(crate) fn upload_begins(self) -> usize {
        self.upload_begins
    }
    pub(crate) fn deletions(self) -> usize {
        self.deletions
    }
    pub(crate) fn ordinary_slots(self) -> usize {
        self.ordinary_slots
    }
    pub(crate) fn control_slots(self) -> usize {
        self.control_slots
    }
    pub(crate) fn app_calls_per_socket(self) -> usize {
        self.app_calls_per_socket
    }
    /// One less than the socket's app lane, so a mount's waiting reviews
    /// leave a slot another app can take.
    pub(crate) fn app_calls_per_mount(self) -> usize {
        self.app_calls_per_socket - 1
    }
    /// Room for every ordinary slot's response and every app call's, which
    /// share the ordinary lane.
    pub(crate) fn ordinary_lane(self) -> usize {
        self.ordinary_slots + self.app_calls_per_socket
    }
    /// The control lane holds one frame per control slot.
    pub(crate) fn control_lane(self) -> usize {
        self.control_slots
    }
    pub(crate) fn read_work_budget(self) -> Duration {
        self.read_work_budget
    }
}

fn admit(count: u64, invalid: InvalidOperationalLimits) -> Result<usize, InvalidOperationalLimits> {
    if !(1..=MAX_ADMISSION).contains(&count) {
        return Err(invalid);
    }
    usize::try_from(count).map_err(|_| invalid)
}

fn app_calls(count: u64) -> Result<usize, InvalidOperationalLimits> {
    if (2..=MAX_ADMISSION).contains(&count) {
        usize::try_from(count).map_err(|_| InvalidOperationalLimits::AppCalls)
    } else {
        Err(InvalidOperationalLimits::AppCalls)
    }
}

fn budget(duration: Duration) -> Result<Duration, InvalidOperationalLimits> {
    if duration.is_zero() || Instant::now().checked_add(duration).is_none() {
        Err(InvalidOperationalLimits::ReadWorkBudget)
    } else {
        Ok(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_counts_the_socket_was_built_with() {
        let limits = OperationalLimits::default();
        assert_eq!(limits.requests(), 128);
        assert_eq!(limits.controls(), 32);
        assert_eq!(limits.record_reads(), 4);
        assert_eq!(limits.upload_begins(), 16);
        assert_eq!(limits.deletions(), 8);
        assert_eq!(limits.ordinary_slots(), 16);
        assert_eq!(limits.control_slots(), 4);
        assert_eq!(limits.app_calls_per_socket(), 4);
        assert_eq!(limits.app_calls_per_mount(), 3);
        assert_eq!(limits.ordinary_lane(), 20);
        assert_eq!(limits.control_lane(), 4);
        assert_eq!(limits.read_work_budget(), READ_WORK_BUDGET);
    }

    #[test]
    fn a_count_of_zero_one_app_call_or_an_unusable_budget_is_refused() {
        let defaults = OperationalLimits::default();
        let mut input = ConfiguredLimits {
            requests: defaults.requests() as u64,
            controls: defaults.controls() as u64,
            record_reads: defaults.record_reads() as u64,
            upload_begins: defaults.upload_begins() as u64,
            deletions: defaults.deletions() as u64,
            ordinary_slots: defaults.ordinary_slots() as u64,
            control_slots: defaults.control_slots() as u64,
            app_calls_per_socket: defaults.app_calls_per_socket() as u64,
            read_work_budget: defaults.read_work_budget(),
        };
        input.requests = 0;
        assert_eq!(
            OperationalLimits::configured(input),
            Err(InvalidOperationalLimits::Requests)
        );
        input.requests = defaults.requests() as u64;
        input.app_calls_per_socket = 1;
        assert_eq!(
            OperationalLimits::configured(input),
            Err(InvalidOperationalLimits::AppCalls)
        );
        input.app_calls_per_socket = defaults.app_calls_per_socket() as u64;
        input.read_work_budget = Duration::ZERO;
        assert_eq!(
            OperationalLimits::configured(input),
            Err(InvalidOperationalLimits::ReadWorkBudget)
        );
        input.read_work_budget = Duration::MAX;
        assert_eq!(
            OperationalLimits::configured(input),
            Err(InvalidOperationalLimits::ReadWorkBudget)
        );
        input.read_work_budget = Duration::from_millis(1);
        assert!(OperationalLimits::configured(input).is_ok());
    }
}
