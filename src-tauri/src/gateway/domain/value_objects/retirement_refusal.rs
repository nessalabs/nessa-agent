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
mod tests {
    use super::*;

    #[test]
    fn every_published_name_is_read_and_only_data_missing_allows_a_stop() {
        let published: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../protocol/defaults/gateway-retirement-refusals.json"
        ))
        .unwrap();
        let names: Vec<&str> = published["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        assert_eq!(names, ["data_missing", "not_confirmed"]);
        assert_eq!(
            names
                .iter()
                .map(|name| RetirementRefusal::named(Some(name)))
                .collect::<Vec<_>>(),
            [
                RetirementRefusal::DataMissing,
                RetirementRefusal::NotConfirmed
            ]
        );
        assert_eq!(
            RetirementRefusal::named(None),
            RetirementRefusal::NotConfirmed
        );
        assert_eq!(
            RetirementRefusal::named(Some("data_missing_later")),
            RetirementRefusal::NotConfirmed
        );
    }
}
