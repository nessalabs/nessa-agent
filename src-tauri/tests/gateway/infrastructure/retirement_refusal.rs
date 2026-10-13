use super::RetirementRefusal;
use serde_json::{from_str, Value};

#[test]
fn every_published_name_is_read_and_only_data_missing_allows_a_stop() {
    let published: Value = from_str(include_str!(
        "../../../../protocol/defaults/gateway-retirement-refusals.json"
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
    for refusal in [
        RetirementRefusal::DataMissing,
        RetirementRefusal::NotConfirmed,
    ] {
        assert!(names.contains(&refusal.name()), "{refusal:?} is published");
        assert_eq!(RetirementRefusal::named(Some(refusal.name())), refusal);
    }
    assert_eq!(
        RetirementRefusal::named(None),
        RetirementRefusal::NotConfirmed
    );
    assert_eq!(
        RetirementRefusal::named(Some("data_missing_later")),
        RetirementRefusal::NotConfirmed
    );
}
