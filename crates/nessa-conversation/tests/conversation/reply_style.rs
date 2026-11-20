use super::{ReplyStyle, ReplyStyleSettings, RoleStyles, StyleRole, MAX_REPLY_STYLE_BYTES};
use crate::domain::{ChannelId, SettingsScope};

fn channel() -> SettingsScope {
    SettingsScope::Channel(ChannelId::new("ch_aaaaaaaaaa").unwrap())
}

#[test]
fn nothing_chosen_is_the_plain_style_for_each_role() {
    let settings = ReplyStyleSettings::new(channel(), RoleStyles::default(), RoleStyles::default());
    for role in StyleRole::ALL {
        assert_eq!(settings.effective(role), ReplyStyle::Plain);
    }
    let coordinator = ReplyStyle::Plain
        .instructions(StyleRole::Coordinator)
        .unwrap();
    let thread = ReplyStyle::Plain.instructions(StyleRole::Thread).unwrap();
    assert!(coordinator.starts_with("How to write to the person:"));
    assert!(coordinator.contains("Be very short"));
    assert!(thread.contains("name the files"));
    assert!(!thread.contains("Be very short"));
}

#[test]
fn a_channel_choice_replaces_the_nessa_level_for_its_role_only() {
    let nessa = RoleStyles::default().with(StyleRole::Thread, ReplyStyle::custom("Terse."));
    let own = RoleStyles::default().with(StyleRole::Coordinator, Some(ReplyStyle::Off));
    let settings = ReplyStyleSettings::new(channel(), nessa, own);
    assert_eq!(settings.effective(StyleRole::Coordinator), ReplyStyle::Off);
    assert_eq!(
        settings.effective(StyleRole::Thread),
        ReplyStyle::Custom("Terse.".into())
    );
    assert_eq!(ReplyStyle::Off.instructions(StyleRole::Thread), None);
}

#[test]
fn clearing_a_channel_choice_inherits_again() {
    let own = RoleStyles::default().with(StyleRole::Thread, Some(ReplyStyle::Off));
    let cleared = own.with(StyleRole::Thread, None);
    let settings = ReplyStyleSettings::new(channel(), RoleStyles::default(), cleared);
    assert_eq!(settings.effective(StyleRole::Thread), ReplyStyle::Plain);
}

#[test]
fn a_global_read_has_no_channel_choices() {
    let own = RoleStyles::default().with(StyleRole::Thread, Some(ReplyStyle::Off));
    let settings = ReplyStyleSettings::new(SettingsScope::Global, RoleStyles::default(), own);
    assert_eq!(settings.channel(), &RoleStyles::default());
    assert_eq!(settings.effective(StyleRole::Thread), ReplyStyle::Plain);
}

#[test]
fn custom_text_is_trimmed_and_bounded() {
    assert_eq!(
        ReplyStyle::custom("  Be brief.  "),
        Some(ReplyStyle::Custom("Be brief.".into()))
    );
    assert_eq!(ReplyStyle::custom("   "), None);
    assert!(ReplyStyle::custom(&"a".repeat(MAX_REPLY_STYLE_BYTES)).is_some());
    assert_eq!(
        ReplyStyle::custom(&"a".repeat(MAX_REPLY_STYLE_BYTES + 1)),
        None
    );
    assert_eq!(
        StyleRole::parse("coordinator"),
        Some(StyleRole::Coordinator)
    );
    assert_eq!(StyleRole::parse("owner"), None);
}
