use super::support::*;

#[test]
fn prompt_sources_require_a_name_for_every_kind() {
    for kind in [
        PromptSourceKind::Core,
        PromptSourceKind::Plugin,
        PromptSourceKind::Mcp,
        PromptSourceKind::Skill,
        PromptSourceKind::User,
    ] {
        for name in ["", " ", "\n\t"] {
            assert_eq!(
                PromptSource::new(kind, name),
                Err(ExecutionError::EmptyValue("prompt source name"))
            );
        }
        let source = PromptSource::new(kind, " α/source ").unwrap();
        assert_eq!(source.kind(), kind);
        assert_eq!(source.name(), " α/source ");
    }
}

#[test]
fn prompt_construction_rejects_blank_completed_content() {
    let core = PromptSource::new(PromptSourceKind::Core, "system").unwrap();
    for builder in [
        SystemPromptBuilder::new(),
        SystemPromptBuilder::default()
            .text(core.clone(), " ")
            .text(core.clone(), "\n"),
    ] {
        assert_eq!(
            builder.build(),
            Err(ExecutionError::EmptyValue("prompt text"))
        );
    }
    for contributions in [vec![], vec![PromptContribution::new(core, "\t")]] {
        assert_eq!(
            SystemPrompt::new(contributions),
            Err(ExecutionError::EmptyValue("prompt text"))
        );
    }
}

#[test]
fn prompt_builder_preserves_each_contribution_and_exact_text_in_append_order() {
    let core = PromptSource::new(PromptSourceKind::Core, "system").unwrap();
    let plugin = PromptSource::new(PromptSourceKind::Plugin, "reviewer").unwrap();
    let mcp = PromptSource::new(PromptSourceKind::Mcp, "workspace").unwrap();
    let skill = PromptSource::new(PromptSourceKind::Skill, "rust").unwrap();
    let expected = vec![
        PromptContribution::new(core.clone(), "Instructions: α"),
        PromptContribution::new(plugin.clone(), "\n\n"),
        PromptContribution::new(plugin.clone(), "Review tools"),
        PromptContribution::new(mcp.clone(), "\nWorkspace: β"),
        PromptContribution::new(skill.clone(), "\nUse Rust"),
        PromptContribution::new(plugin.clone(), ""),
    ];
    let prompt = SystemPromptBuilder::new()
        .text(core.clone(), "Instructions: α")
        .text(plugin.clone(), "\n\n")
        .text(plugin.clone(), "Review tools")
        .text(mcp, "\nWorkspace: β")
        .text(skill, "\nUse Rust")
        .text(plugin, "")
        .build()
        .unwrap();
    assert_eq!(
        prompt.text().as_str(),
        "Instructions: α\n\nReview tools\nWorkspace: β\nUse Rust"
    );
    assert_eq!(
        prompt
            .contributions()
            .map(|part| (part.source(), part.text()))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|part| (part.source(), part.text()))
            .collect::<Vec<_>>()
    );
    assert_eq!(prompt.contributions().next().unwrap().source(), &core);
    assert_eq!(prompt, SystemPrompt::new(expected).unwrap());
    assert_eq!(prompt.clone(), prompt);
    // Equal provider text does not erase differences in provenance.
    let single_source = SystemPromptBuilder::new()
        .text(core.clone(), prompt.text().as_str())
        .build()
        .unwrap();
    assert_eq!(single_source.text(), prompt.text());
    assert_ne!(single_source, prompt);
    let independent = SystemPromptBuilder::new()
        .text(core, "independent")
        .build()
        .unwrap();
    assert_eq!(independent.contributions().len(), 1);
    assert_eq!(prompt.contributions().len(), 6);
}

/// A carried turn holds text that is not blank, each part within its bound;
/// one read from records is cut at a character boundary instead.
#[test]
fn a_carried_turn_holds_bounded_text_and_a_clipped_one_keeps_its_start() {
    use nessa_sdk::domain::agent_execution::prompts::CarriedTurn;
    let max = CarriedTurn::MAX_PART_BYTES;
    assert_eq!(
        CarriedTurn::new(None, Some(" \n")),
        Err(ExecutionError::EmptyValue("carried turn"))
    );
    for (said, reply, field) in [
        (Some("x".repeat(max + 1)), None, "carried turn text"),
        (None, Some("x".repeat(max + 1)), "carried turn reply"),
    ] {
        assert_eq!(
            CarriedTurn::new(said.as_deref(), reply.as_deref()),
            Err(ExecutionError::ValueTooLong {
                field,
                max_bytes: max
            })
        );
    }
    let turn = CarriedTurn::new(Some("said"), Some("x".repeat(max).as_str())).unwrap();
    assert_eq!(turn.said(), Some("said"));
    assert_eq!(turn.text_bytes(), 4 + max);
    let only = CarriedTurn::new(Some(" "), Some("reply")).unwrap();
    assert_eq!((only.said(), only.reply()), (None, Some("reply")));

    // "é" is two bytes, so an odd bound falls inside one and steps back.
    let long = format!("a{}", "é".repeat(max));
    let clipped = CarriedTurn::clipped(Some(&long), Some(" short ")).unwrap();
    assert_eq!(clipped.said().unwrap().len(), max - 1);
    assert!(long.starts_with(clipped.said().unwrap()));
    assert_eq!(clipped.reply(), Some("short"));
    assert_eq!(CarriedTurn::clipped(Some(" "), None), None);
    assert_eq!(CarriedTurn::clipped(None, None), None);
}

/// History keeps the newest turns within both bounds and counts the rest;
/// one read back past either bound is refused.
#[test]
fn carried_history_keeps_the_newest_within_its_bounds_and_refuses_more() {
    use nessa_sdk::domain::agent_execution::prompts::{CarriedHistory, CarriedTurn};
    let empty = CarriedHistory::newest(0, Vec::new());
    assert!(empty.is_empty());
    assert_eq!(empty.text_bytes(), 0);
    assert!(!CarriedHistory::newest(1, Vec::new()).is_empty());

    // The byte bound stops it before the turn count does.
    let big = || CarriedTurn::new(Some(&"x".repeat(CarriedTurn::MAX_PART_BYTES)), None).unwrap();
    let fits = CarriedHistory::MAX_TEXT_BYTES / CarriedTurn::MAX_PART_BYTES;
    let history = CarriedHistory::newest(u32::MAX, (0..fits + 3).map(|_| big()).collect());
    assert_eq!(history.turns().len(), fits);
    assert_eq!(history.omitted(), u32::MAX);
    assert_eq!(history.text_bytes(), CarriedHistory::MAX_TEXT_BYTES);
    assert!(!history.is_empty());

    let small = || CarriedTurn::new(Some("t"), None).unwrap();
    assert_eq!(
        CarriedHistory::new(
            (0..=CarriedHistory::MAX_TURNS).map(|_| small()).collect(),
            0
        ),
        Err(ExecutionError::TooManyValues {
            field: "carried history turns",
            max: CarriedHistory::MAX_TURNS
        })
    );
    assert_eq!(
        CarriedHistory::new((0..=fits).map(|_| big()).collect(), 0),
        Err(ExecutionError::ValueTooLong {
            field: "carried history",
            max_bytes: CarriedHistory::MAX_TEXT_BYTES
        })
    );
    let read = CarriedHistory::new(vec![small()], 7).unwrap();
    assert_eq!((read.turns().len(), read.omitted()), (1, 7));
}
