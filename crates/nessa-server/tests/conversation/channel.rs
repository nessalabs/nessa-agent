//! A channel holds only values it can keep: an id the gateway made, a name a
//! sidebar can show, absolute paths, and notes and resources within bounds.
//! What its agents are told about it is one text, the same for every agent.
use super::*;
use crate::conversation_test_support::absolute;

fn path(text: &str) -> ChannelPath {
    ChannelPath::new(PathBuf::from(text)).unwrap()
}

fn parts(folder: ChannelFolder) -> ChannelParts {
    ChannelParts {
        name: ChannelName::new("design-review").unwrap(),
        folder,
        defaults: ChannelDefaults {
            agent: AgentId::Codex,
            model: ConversationModelId::new("gpt-5.6-terra").unwrap(),
            mode: ConversationApprovalMode::Auto,
        },
        notes: ChannelNotes::new("Check every export.").unwrap(),
        resources: ChannelResources::new(vec![path(&absolute("/home/me/brand-kit"))]).unwrap(),
    }
}

fn channel(folder: ChannelFolder) -> Channel {
    Channel::new(
        ChannelId::from_random([1; 10]),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        parts(folder),
        "request-1".into(),
        1_000,
    )
    .unwrap()
}

#[test]
fn an_id_is_the_default_or_ch_and_ten_base32_characters() {
    assert!(ChannelId::new("default").unwrap().is_default());
    assert_eq!(
        ChannelId::new("ch_abcdefgh27").unwrap().as_str(),
        "ch_abcdefgh27"
    );
    for refused in [
        "",
        "ch_",
        "ch_abcdefgh2",
        "ch_abcdefgh278",
        "ch_ABCDEFGH27",
        "ch_abcdefgh18",
        "xx_abcdefgh27",
    ] {
        assert_eq!(
            ChannelId::new(refused),
            Err(ChannelRefusal::Id),
            "{refused}"
        );
    }
}

#[test]
fn a_random_id_reads_back_as_an_id() {
    for bytes in [
        [0; 10],
        [0xff; 10],
        [7, 31, 32, 63, 64, 95, 128, 200, 255, 1],
    ] {
        let id = ChannelId::from_random(bytes);
        assert_eq!(ChannelId::new(id.as_str()).unwrap(), id);
        assert!(!id.is_default());
    }
}

#[test]
fn a_name_is_trimmed_and_bounded() {
    assert_eq!(ChannelName::new("  design  ").unwrap().as_str(), "design");
    assert_eq!(ChannelName::new("   "), Err(ChannelRefusal::Name));
    assert_eq!(ChannelName::new("a\nb"), Err(ChannelRefusal::Name));
    let longest = "é".repeat(MAX_CHANNEL_NAME_CHARS);
    assert!(ChannelName::new(&longest).is_ok());
    assert_eq!(
        ChannelName::new(&format!("{longest}é")),
        Err(ChannelRefusal::Name)
    );
}

#[test]
fn notes_keep_line_breaks_and_refuse_other_control_characters() {
    assert_eq!(
        ChannelNotes::new("one\n\ttwo").unwrap().as_str(),
        "one\n\ttwo"
    );
    assert!(ChannelNotes::new("").unwrap().is_empty());
    assert_eq!(ChannelNotes::new("bell\u{7}"), Err(ChannelRefusal::Notes));
    assert_eq!(
        ChannelNotes::new(&"a".repeat(MAX_CHANNEL_NOTES_BYTES + 1)),
        Err(ChannelRefusal::Notes)
    );
}

#[test]
fn a_path_is_absolute() {
    assert_eq!(
        ChannelPath::new(PathBuf::from("relative/dir")),
        Err(ChannelRefusal::Path)
    );
    assert_eq!(
        ChannelPath::new(PathBuf::from(format!(
            "/{}",
            "a".repeat(MAX_CHANNEL_PATH_BYTES)
        ))),
        Err(ChannelRefusal::Path)
    );
}

#[test]
fn resources_are_bounded_and_never_named_twice() {
    assert_eq!(
        ChannelResources::new(vec![path(&absolute("/a")), path(&absolute("/a"))]),
        Err(ChannelRefusal::Resources)
    );
    let many = (0..=MAX_CHANNEL_RESOURCES)
        .map(|n| path(&absolute(&format!("/r{n}"))))
        .collect();
    assert_eq!(ChannelResources::new(many), Err(ChannelRefusal::Resources));
    let long = |n: usize| {
        path(&absolute(&format!(
            "/{n}{}",
            "a".repeat(MAX_CHANNEL_PATH_BYTES - 8)
        )))
    };
    let past_bytes = (0..5).map(long).collect();
    assert_eq!(
        ChannelResources::new(past_bytes),
        Err(ChannelRefusal::Resources)
    );
}

#[test]
fn the_default_channel_is_never_stored() {
    let refused = Channel::new(
        ChannelId::default_channel(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        parts(ChannelFolder {
            path: path(&absolute("/work")),
            made: false,
        }),
        "request-1".into(),
        1,
    );
    assert_eq!(refused, Err(ChannelRefusal::Id));
}

#[test]
fn a_retry_is_the_same_channel_only_when_it_asks_for_the_same_folder() {
    let made = channel(ChannelFolder {
        path: path(&absolute("/data/channels/x")),
        made: true,
    });
    let mut asked = ChannelRequest {
        name: made.name().clone(),
        folder: None,
        defaults: made.defaults().clone(),
        notes: made.notes().clone(),
        resources: made.resources().clone(),
    };
    assert!(made.made_of(&asked));
    asked.defaults.mode = ConversationApprovalMode::Ask;
    assert!(
        !made.made_of(&asked),
        "another default mode is another channel"
    );
    asked.defaults.mode = ConversationApprovalMode::Auto;
    asked.folder = Some(path(&absolute("/data/channels/x")));
    assert!(!made.made_of(&asked), "a chosen folder is not the made one");

    let chosen = channel(ChannelFolder {
        path: path(&absolute("/repo")),
        made: false,
    });
    asked.folder = Some(path(&absolute("/repo")));
    assert!(chosen.made_of(&asked));
    asked.folder = None;
    assert!(!chosen.made_of(&asked));
}

#[test]
fn agents_are_told_the_name_folder_notes_and_resources() {
    let made = channel(ChannelFolder {
        path: path(&absolute("/repo")),
        made: false,
    });
    let text = made.instructions();
    assert!(text.contains("\"design-review\""), "{text}");
    assert!(text.contains("Its folder is /repo."), "{text}");
    assert!(text.contains("Check every export."), "{text}");
    assert!(text.contains("\n- /home/me/brand-kit"), "{text}");
}
