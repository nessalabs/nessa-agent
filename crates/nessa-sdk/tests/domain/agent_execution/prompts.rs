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
