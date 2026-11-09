use super::resolve_inherited;

#[test]
fn the_nearest_present_choice_wins_through_multiple_scopes() {
    let default = "built-in";
    assert_eq!(resolve_inherited(default, [None, None, None]), default);
    assert_eq!(
        resolve_inherited(default, [Some("global"), Some("project"), None]),
        "project"
    );
    let choices = [Some("global"), Some("project"), Some("channel")];
    assert_eq!(resolve_inherited(default, choices), "channel");
    assert_eq!(
        resolve_inherited(default, [None, Some("project"), None]),
        "project"
    );
}

#[test]
fn an_explicit_nullable_value_does_not_mean_inherit() {
    let default = Some(3600_u32);
    assert_eq!(
        resolve_inherited(default, [Some(Some(7200)), None]),
        Some(7200)
    );
    assert_eq!(
        resolve_inherited(default, [Some(Some(7200)), Some(None)]),
        None
    );
    assert_eq!(resolve_inherited(default, [None, None]), default);
}

#[test]
fn owned_setting_values_can_replace_their_parent() {
    let effective = resolve_inherited(
        String::from("built-in"),
        [None, Some(String::from("channel"))],
    );
    assert_eq!(effective, "channel");
}
