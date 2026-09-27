//! Prompt values cannot amplify a byte budget through caller spare capacity.
use super::{PromptContribution, PromptSource, PromptSourceKind, PromptText, SystemPrompt};

#[test]
fn prompt_storage_compacts_owned_capacity_and_preserves_exact_text() {
    let mut text = String::with_capacity(8 * 1024 * 1024);
    text.push_str("  exact é  ");
    let prompt = PromptText::new(text).unwrap();
    assert_eq!(prompt.as_str(), "  exact é  ");
    for value in [prompt.clone(), prompt] {
        let retained = value.0.into_string();
        assert_eq!(retained.capacity(), retained.len());
    }
}

#[test]
fn system_prompt_compacts_contribution_slots_and_every_owned_fragment() {
    let mut source = String::with_capacity(1024 * 1024);
    source.push_str(" source ");
    let source = PromptSource::new(PromptSourceKind::Core, source).unwrap();
    let mut fragment = String::with_capacity(1024 * 1024);
    fragment.push_str(" exact é ");
    let mut contributions = Vec::with_capacity(100_000);
    contributions.push(PromptContribution::new(source, fragment));
    let prompt = SystemPrompt::new(contributions).unwrap();
    assert_eq!(prompt.text().as_str(), " exact é ");
    for value in [prompt.clone(), prompt] {
        let contributions = value.contributions.into_vec();
        assert_eq!(contributions.capacity(), 1);
        let contribution = contributions.into_iter().next().unwrap();
        assert_eq!(contribution.start, 0);
        assert_eq!(contribution.end, " exact é ".len());
        let name = contribution.source.name.into_string();
        assert_eq!(name.capacity(), name.len());
        assert_eq!(name, " source ");
    }
}

#[test]
fn assembled_contributions_share_one_utf8_backing_in_original_and_clone() {
    let fragments = ["α", "", "\n\n", "é🙂", " "];
    let contributions = fragments
        .iter()
        .enumerate()
        .map(|(index, text)| {
            PromptContribution::new(
                PromptSource::new(PromptSourceKind::Plugin, format!("part-{index}")).unwrap(),
                *text,
            )
        })
        .collect();
    let prompt = SystemPrompt::new(contributions).unwrap();
    let clone = prompt.clone();
    assert_eq!(prompt, clone);
    assert_ne!(
        prompt.text().as_str().as_ptr(),
        clone.text().as_str().as_ptr()
    );
    for value in [&prompt, &clone] {
        let assembled = value.text().as_str();
        assert_eq!(assembled, "α\n\né🙂 ");
        let mut offset = 0;
        for (index, (view, expected)) in value.contributions().zip(fragments).enumerate() {
            assert_eq!(view.text(), expected);
            assert_eq!(view.source().name(), format!("part-{index}"));
            assert_eq!(view.text().as_ptr(), assembled[offset..].as_ptr());
            offset += expected.len();
        }
        assert_eq!(offset, assembled.len());
        let mut iter = value.contributions();
        assert_eq!(iter.len(), 5);
        assert_eq!(iter.next_back().unwrap().text(), " ");
        assert_eq!(iter.len(), 4);
        assert_eq!(iter.next().unwrap().text(), "α");
        let expected = assembled.len()
            + value.contributions.len() * std::mem::size_of::<super::PromptContributionRange>()
            + value
                .contributions
                .iter()
                .map(|part| part.source.name.len())
                .sum::<usize>();
        assert_eq!(value.payload_bytes(), expected);
    }
}

#[test]
fn large_assembled_prompt_retains_instruction_bytes_once() {
    let text = "é".repeat(1024 * 1024);
    let prompt = SystemPrompt::new(vec![
        PromptContribution::new(
            PromptSource::new(PromptSourceKind::Core, "core").unwrap(),
            text.clone(),
        ),
        PromptContribution::new(
            PromptSource::new(PromptSourceKind::Skill, "skill").unwrap(),
            text,
        ),
    ])
    .unwrap();
    let bytes = 4 * 1024 * 1024;
    assert_eq!(prompt.text().as_str().len(), bytes);
    assert_eq!(
        prompt.payload_bytes(),
        bytes + 2 * std::mem::size_of::<super::PromptContributionRange>() + 4 + 5
    );
    assert_eq!(
        prompt
            .contributions()
            .map(|part| part.text().len())
            .sum::<usize>(),
        bytes
    );
}
