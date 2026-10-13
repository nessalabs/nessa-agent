//! Why a running gateway would not retire (ADR 221), by the names in
//! `protocol/defaults/gateway-retirement-refusals.json`, which the gateway
//! writes. The host decides what to do from the name alone.

/// The reason a retirement did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetirementRefusal {
    /// The gateway's own conversation data is gone, so the evidence the refusal
    /// protects is gone too, and the host may stop the gateway itself.
    DataMissing,
    /// Anything else, including a gateway that predates refusal names.
    NotConfirmed,
}

impl RetirementRefusal {
    /// The published name, for the record of a refused retirement.
    pub fn name(self) -> &'static str {
        match self {
            Self::DataMissing => "data_missing",
            Self::NotConfirmed => "not_confirmed",
        }
    }

    /// The refusal a result names. A result with no name, or one this build does
    /// not know, is `NotConfirmed`: nothing is stopped on a reason the host
    /// cannot read.
    pub fn named(name: Option<&str>) -> Self {
        match name {
            Some("data_missing") => Self::DataMissing,
            _ => Self::NotConfirmed,
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/retirement_refusal.rs"]
mod tests;
