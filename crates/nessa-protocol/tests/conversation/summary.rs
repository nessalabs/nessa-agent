//! A conversation's summary rules, tested without storage, a runtime, or an agent.
use super::{ConversationPreview, ConversationSummary, ConversationTitle};
use crate::conversation::view::{
    AskEffort, AskUndo, AutoPick, OwnerAsk, OwnerAskChoice, OwnerDecisionOutcome, MAX_ASK_CHOICES,
    MAX_ASK_DETAILS, MAX_ASK_DETAIL_BYTES, MAX_ASK_LABEL_BYTES, MAX_ASK_LINE_BYTES,
    MAX_ASK_QUESTION_BYTES, MAX_DECISION_ANSWER_BYTES, MAX_DECISION_OUTCOME_JSON_BYTES,
};

fn preview(text: &str) -> Option<String> {
    ConversationPreview::from_text(text).map(|preview| preview.as_str().to_owned())
}

#[test]
fn a_reply_previews_as_one_line_of_plain_text() {
    assert_eq!(
        preview(
            "## Booked\n\n- Confirmation **K7Q2PX**, see [the email](https://x.test)\n- Run `pnpm dev` in snake_case_dir"
        )
        .as_deref(),
        Some("Booked Confirmation K7Q2PX, see the email Run pnpm dev in snake_case_dir")
    );
    for (text, expected) in [
        ("# One\n###### Six\n####### Seven", "One Six ####### Seven"),
        (
            "> quoted\n   > indented\n    > code",
            "quoted indented > code",
        ),
        ("* a\n+ b\n1. c\n12) d\n> - nested", "a b c d nested"),
        ("![a cat](cat.png) and [x]", "a cat and [x]"),
        (
            "__bold__ *it* _it_ ~~gone~~ ``co`de``",
            "bold it it gone co`de",
        ),
        ("[**bold link**](https://x.test)", "bold link"),
        // Not marks: inside a word, around whitespace, or never closed.
        (
            "2*3*4 a * b * c snake_case *open",
            "2*3*4 a * b * c snake_case *open",
        ),
        ("__init__ and ***three***", "init and ***three***"),
        ("#hashtag", "#hashtag"),
        ("tab\there\u{7}bell\u{2028}line", "tab here bell line"),
    ] {
        assert_eq!(preview(text).as_deref(), Some(expected), "{text:?}");
    }
    // Nothing a row could show.
    for text in ["", " \n\t", "- ", "## ", "> "] {
        assert_eq!(preview(text), None, "{text:?}");
    }
}

#[test]
fn a_preview_leaves_out_tables_and_code() {
    assert_eq!(
        preview(
            "Tables: two options\n\n| Option | Price |\n| --- | ---: |\n| Single | $16 |\n\nPick one."
        )
        .as_deref(),
        Some("Tables: two options Pick one.")
    );
    assert_eq!(
        preview("Run this:\n\n```sh\npnpm dev\n| not a table |\n```\nThen reload.").as_deref(),
        Some("Run this: Then reload.")
    );
    // A delimiter row with no leading pipe is still a table's.
    assert_eq!(preview("a | b\n---|---\nok").as_deref(), Some("a | b ok"));
    // A reply that opens with a block still previews the prose after it.
    assert_eq!(
        preview("```\nls\n```\nDone, two files.").as_deref(),
        Some("Done, two files.")
    );
    // A reply of a table alone has nothing a row could show.
    assert_eq!(preview("| a | b |\n|---|---|\n| 1 | 2 |"), None);
}

#[test]
fn a_preview_is_cut_on_a_character_boundary_at_its_byte_bound() {
    let text = "€".repeat(300);
    let preview = ConversationPreview::from_text(&text).unwrap();
    assert_eq!(preview.as_str(), "€".repeat(170));
    assert!(preview.as_str().len() <= ConversationPreview::MAX_BYTES);
    // A cut that lands on a space does not leave it trailing.
    let words = format!("{} tail", "a".repeat(511));
    assert_eq!(
        ConversationPreview::from_text(&words).unwrap().as_str(),
        "a".repeat(511)
    );
}

#[test]
fn a_title_is_the_first_line_of_the_first_message() {
    for (text, file, expected) in [
        ("  Plan the trip\nwith details", None, "Plan the trip"),
        ("\n\n  spaced    out  words ", None, "spaced out words"),
        ("", Some("report.pdf"), "report.pdf"),
        ("   ", Some("report.pdf"), "report.pdf"),
        ("", None, "Image"),
        // Read as plain text, like a preview: it is shown in the same list.
        ("# Heading", None, "Heading"),
        (
            "Plan a **3-day** trip\nin `Lisbon`",
            None,
            "Plan a 3-day trip",
        ),
        ("", Some("snake_case_notes.md"), "snake_case_notes.md"),
        // A line is the first only if something of it is kept: control
        // characters are not, alone or once a marker is dropped.
        ("\u{7}\nHello there", None, "Hello there"),
        ("- \u{7}\nHello there", None, "Hello there"),
        ("- \u{7}", Some("report.pdf"), "report.pdf"),
        ("\u{1b}\u{7}", None, "Image"),
    ] {
        assert_eq!(
            ConversationTitle::for_message(text, file).as_str(),
            expected,
            "{text:?}"
        );
    }
    let long = ConversationTitle::for_message(&"é".repeat(60), None);
    assert_eq!(long.as_str().chars().count(), ConversationTitle::MAX_CHARS);
    // A cut that lands on a space does not leave it trailing.
    let words = format!("{} tail", "a".repeat(47));
    assert_eq!(
        ConversationTitle::for_message(&words, None).as_str(),
        "a".repeat(47)
    );
}

#[test]
fn a_title_and_a_preview_read_the_same_bounded_opening() {
    let title = |text: &str| {
        ConversationTitle::for_message(text, None)
            .as_str()
            .to_owned()
    };
    // Lines with nothing visible once their marks are dropped are passed
    // over however many there are, by both.
    for text in [
        format!("{}Late line", "\n".repeat(9000)),
        format!("{}Late line", "> \n".repeat(3000)),
        format!("{}Late line", "\n \u{7}".repeat(9000)),
        format!("{}Late line", "- \u{7}\n".repeat(3000)),
        format!("{}Late line", "[](x)\n".repeat(100)),
        // More of them than one bound's worth: read as plain text, not
        // taken for visible because they hold characters.
        format!("{}Late line", "[](x)\n".repeat(2000)),
    ] {
        assert_eq!(title(&text), "Late line");
        assert_eq!(preview(&text).as_deref(), Some("Late line"));
    }
    // The markers before a line's first words are passed over too.
    assert_eq!(title(&format!("{}Late", "- ".repeat(5000))), "Late");
    // A first line is read only to the bound: its words past it are not.
    let long = format!("a{}Late", " ".repeat(9000));
    assert_eq!(title(&long), "a");
    assert_eq!(preview(&long).as_deref(), Some("a"));
    // Lines that say nothing only once read as plain text are read for at
    // most the bound in all, then the opening starts where they do.
    // Eight bytes a line, so the opening's bound ends on a line.
    let links = format!("{}Late", "[](xyz)\n".repeat(5000));
    assert_eq!(
        ConversationTitle::for_message(&links, Some("notes.md")).as_str(),
        "notes.md"
    );
    assert_eq!(preview(&links), None);
}

#[test]
fn stored_values_must_be_what_the_rules_produce() {
    assert!(ConversationTitle::new("Plan the trip").is_ok());
    assert!(ConversationTitle::new(&"a".repeat(ConversationTitle::MAX_CHARS)).is_ok());
    for title in ["", " padded", "two  spaces", "line\nbreak", &"a".repeat(49)] {
        assert!(ConversationTitle::new(title).is_err(), "{title:?}");
    }
    assert!(ConversationPreview::new(&"a".repeat(ConversationPreview::MAX_BYTES)).is_ok());
    for preview in ["", "trailing ", "tab\there", &"a".repeat(513)] {
        assert!(ConversationPreview::new(preview).is_err(), "{preview:?}");
    }
}

#[test]
fn a_summary_keeps_its_first_title_and_follows_what_was_said_last() {
    let first = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    assert_eq!(first.title().unwrap().as_str(), "Plan the trip");
    assert_eq!(first.preview().unwrap().as_str(), "Plan the trip");
    assert_eq!(first.updated_at_ms(), 10);

    let second = ConversationSummary::after_message(Some(&first), "", Some("a.pdf"), 20);
    assert_eq!(second.title().unwrap().as_str(), "Plan the trip");
    assert_eq!(second.preview().unwrap().as_str(), "Attachment");
    assert_eq!(second.updated_at_ms(), 20);

    let reply = ConversationSummary::after_reply(Some(&second), "**Done.**", 30).unwrap();
    assert_eq!(reply.title().unwrap().as_str(), "Plan the trip");
    assert_eq!(reply.preview().unwrap().as_str(), "Done.");
    assert_eq!(reply.updated_at_ms(), 30);

    // A reply with nothing to show changes nothing.
    assert_eq!(
        ConversationSummary::after_reply(Some(&reply), " ", 40),
        None
    );
    // A reply never titles a conversation that has no title yet.
    let untitled = ConversationSummary::after_reply(None, "hello", 5).unwrap();
    assert_eq!(untitled.title(), None);
    // Its first message does.
    let titled = ConversationSummary::after_message(Some(&untitled), "Name me", None, 6);
    assert_eq!(titled.title().unwrap().as_str(), "Name me");

    // A clock stepped backwards does not make a conversation older.
    let stepped = ConversationSummary::after_message(Some(&reply), "again", None, 1);
    assert_eq!(stepped.updated_at_ms(), 30);
}

#[test]
fn a_new_message_unarchives_and_a_reply_does_not() {
    let said = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    assert!(!said.archived());
    let archived = said.after_archiving(true);
    assert!(archived.archived());
    // Archiving says nothing and changes nothing else, the time included.
    assert_eq!(archived.title(), said.title());
    assert_eq!(archived.preview(), said.preview());
    assert_eq!(archived.updated_at_ms(), 10);

    // A turn that finishes after the archive is not somebody talking in it.
    let replied = ConversationSummary::after_reply(Some(&archived), "Done.", 20).unwrap();
    assert!(replied.archived());
    // A message is.
    let talked = ConversationSummary::after_message(Some(&replied), "again", None, 30);
    assert!(!talked.archived());
    assert_eq!(talked.title(), said.title());
    let unarchived = archived.after_archiving(false);
    assert!(!unarchived.archived());
    assert_eq!(unarchived.updated_at_ms(), 10);
}

#[test]
fn a_renamed_title_sticks_through_later_messages() {
    let said = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    let archived = said.after_archiving(true);
    let renamed = archived.after_renaming(ConversationTitle::new("Lisbon in May").unwrap());
    assert_eq!(renamed.title().unwrap().as_str(), "Lisbon in May");
    // Renaming says nothing and changes nothing else, the time included.
    assert_eq!(renamed.preview(), said.preview());
    assert_eq!(renamed.updated_at_ms(), 10);
    assert!(renamed.archived());

    let replied = ConversationSummary::after_reply(Some(&renamed), "Done.", 20).unwrap();
    assert_eq!(replied.title().unwrap().as_str(), "Lisbon in May");
    let talked = ConversationSummary::after_message(Some(&replied), "And Porto?", None, 30);
    assert_eq!(talked.title().unwrap().as_str(), "Lisbon in May");
    assert_eq!(talked.preview().unwrap().as_str(), "And Porto?");
}

fn asked(ask_id: &str, at_ms: u64) -> crate::conversation::view::OwnerAsk {
    crate::conversation::view::OwnerAsk {
        ask_id: ask_id.into(),
        question: "Ship it?".into(),
        choices: vec![
            crate::conversation::view::OwnerAskChoice {
                label: "Yes".into(),
                consequence: "It ships.".into(),
                undo: None,
                effort: None,
                pros: Vec::new(),
                cons: Vec::new(),
            },
            crate::conversation::view::OwnerAskChoice {
                label: "No".into(),
                consequence: "It waits.".into(),
                undo: None,
                effort: None,
                pros: Vec::new(),
                cons: Vec::new(),
            },
        ],
        recommended: 0,
        reason: "It is ready.".into(),
        asked_at_ms: at_ms,
        summary: None,
        auto_pick: None,
    }
}

#[test]
fn an_ask_lifts_its_conversation_to_the_top_of_the_list() {
    let said =
        ConversationSummary::after_message(None, "Plan the trip", None, 10).after_archiving(true);
    let asking = ConversationSummary::after_asking(Some(&said), asked("a", 40));
    assert!(!asking.archived());
    assert_eq!(asking.updated_at_ms(), 40);
    assert_eq!(asking.title(), said.title());
    assert_eq!(asking.preview(), said.preview());
    assert_eq!(asking.ask().map(|ask| ask.ask_id.as_str()), Some("a"));
    // What else happens to the summary keeps the ask; a fork does not.
    let replied = ConversationSummary::after_reply(Some(&asking), "Done.", 50).unwrap();
    assert!(replied.ask().is_some());
    assert!(replied.after_archiving(true).ask().is_some());
    assert!(ConversationSummary::after_forking(&asking, 60)
        .ask()
        .is_none());
}

#[test]
fn an_ask_goes_only_with_the_message_that_answers_it() {
    let asking = ConversationSummary::after_asking(None, asked("a", 10));
    // A message that answers nothing, or answers an earlier ask, keeps it.
    let kept = ConversationSummary::after_message(Some(&asking), "hm", None, 20)
        .answering(None, "hm", false);
    assert!(kept.ask().is_some());
    assert!(kept.settled().is_none());
    let stale = ConversationSummary::after_message(Some(&asking), "hm", None, 20).answering(
        Some("earlier"),
        "hm",
        false,
    );
    assert!(stale.ask().is_some());
    assert!(stale.settled().is_none());
    let answered = ConversationSummary::after_message(Some(&asking), "Yes", None, 30).answering(
        Some("a"),
        "Yes",
        false,
    );
    assert!(answered.ask().is_none());
    let settled = answered.settled().expect("the answered ask is settled");
    assert_eq!(settled.ask.ask_id, "a");
    assert_eq!(
        settled.outcome,
        OwnerDecisionOutcome::Decided {
            text: "Yes".into(),
            choice: Some(0),
            at_ms: 30,
            auto: false,
        }
    );
    // Settling is the change's alone: what is made of the summary next does
    // not settle it again.
    assert!(ConversationSummary::after_reply(Some(&answered), "Ok.", 40)
        .unwrap()
        .settled()
        .is_none());
    assert!(answered.after_archiving(true).settled().is_none());
}

#[test]
fn a_newer_ask_settles_the_one_it_replaces() {
    let first = ConversationSummary::after_asking(None, asked("a", 10));
    assert!(first.settled().is_none());
    let second = ConversationSummary::after_asking(Some(&first), asked("b", 20));
    let settled = second.settled().expect("the first ask is settled");
    assert_eq!(settled.ask.ask_id, "a");
    assert_eq!(
        settled.outcome,
        OwnerDecisionOutcome::Replaced {
            by: "b".into(),
            at_ms: 20,
        }
    );
    // The same ask again settles nothing.
    assert!(
        ConversationSummary::after_asking(Some(&second), asked("b", 30))
            .settled()
            .is_none()
    );
}

#[test]
fn an_answer_names_the_choice_it_is_and_keeps_a_bounded_text() {
    let ask = asked("a", 10);
    let answer = |text: &str| OwnerDecisionOutcome::answered(&ask, text, 5);
    let choice = |outcome: OwnerDecisionOutcome| match outcome {
        OwnerDecisionOutcome::Decided { choice, .. } => choice,
        OwnerDecisionOutcome::Replaced { .. } => panic!("an answer is decided"),
    };
    assert_eq!(choice(answer("No")), Some(1));
    assert_eq!(choice(answer("  no ")), Some(1));
    assert_eq!(choice(answer("No. Not this week.")), Some(1));
    assert_eq!(choice(answer("Nope")), None);
    assert_eq!(choice(answer("Not now")), None);
    // One label opening another: the longer one said is that one.
    let mut nested = asked("b", 10);
    nested.choices[0].label = "Keep".into();
    nested.choices[1].label = "Keep. Then delete".into();
    let said = |text: &str| choice(OwnerDecisionOutcome::answered(&nested, text, 5));
    assert_eq!(said("Keep. Then delete"), Some(1));
    assert_eq!(said("Keep. Then delete. Today."), Some(1));
    assert_eq!(said("Keep. For now."), Some(0));
    let long = "é".repeat(MAX_DECISION_ANSWER_BYTES);
    match answer(&long) {
        OwnerDecisionOutcome::Decided { text, .. } => {
            assert!(text.len() <= MAX_DECISION_ANSWER_BYTES);
            assert!(long.starts_with(&text));
        }
        OwnerDecisionOutcome::Replaced { .. } => panic!("an answer is decided"),
    }
}

#[test]
fn an_auto_pick_taken_back_waits_again_unless_something_was_asked_since() {
    let ask = asked("a", 10);
    let waiting = ConversationSummary::after_asking(None, ask.clone());
    let picked = ConversationSummary::after_message(Some(&waiting), "Yes.", None, 20).answering(
        Some("a"),
        "Yes. Picked for you",
        true,
    );
    assert!(picked.ask().is_none());
    let back = ConversationSummary::reopening(Some(&picked), ask.clone()).unwrap();
    assert_eq!(back.ask().map(|ask| ask.ask_id.as_str()), Some("a"));
    assert!(back.reopened());
    assert!(back.settled().is_none());
    assert_eq!(back.title(), picked.title());
    let newer = ConversationSummary::after_asking(Some(&picked), asked("b", 30));
    assert!(ConversationSummary::reopening(Some(&newer), ask.clone()).is_none());
    assert!(ConversationSummary::reopening(None, ask).is_none());
}

#[test]
fn an_answer_of_control_characters_still_fits_where_decisions_are_kept() {
    let ask = asked("a", 10);
    let worst = "\u{1}".repeat(MAX_DECISION_ANSWER_BYTES * 2);
    let outcome = OwnerDecisionOutcome::answered(&ask, &worst, u64::MAX);
    let json = serde_json::to_string(&outcome).unwrap();
    assert!(
        json.len() <= MAX_DECISION_OUTCOME_JSON_BYTES,
        "{} bytes",
        json.len()
    );
}

/// The largest ask an agent can leave, every part at its bound and written
/// in characters JSON escapes, still fits the 8 KiB column it is kept in.
#[test]
fn the_largest_ask_fits_where_asks_are_kept() {
    let quoted = |bytes: usize| "\"".repeat(bytes);
    let ask = OwnerAsk {
        ask_id: "0123456789abcdef0123456789abcdef".into(),
        question: format!("a {}", quoted(MAX_ASK_QUESTION_BYTES - 2)),
        choices: (0..MAX_ASK_CHOICES)
            .map(|index| OwnerAskChoice {
                label: format!("{index}{}", quoted(MAX_ASK_LABEL_BYTES - 1)),
                consequence: quoted(MAX_ASK_LINE_BYTES),
                undo: Some(AskUndo::Costly),
                effort: Some(AskEffort::Medium),
                pros: vec![quoted(MAX_ASK_DETAIL_BYTES); MAX_ASK_DETAILS],
                cons: vec![quoted(MAX_ASK_DETAIL_BYTES); MAX_ASK_DETAILS],
            })
            .collect(),
        recommended: 0,
        reason: quoted(MAX_ASK_LINE_BYTES),
        asked_at_ms: u64::MAX,
        summary: Some(quoted(MAX_ASK_LINE_BYTES)),
        auto_pick: Some(AutoPick::Never),
    };
    assert_eq!(ask.check(), Ok(()));
    let json = serde_json::to_string(&ask).unwrap();
    assert!(json.len() <= 8192, "{} bytes", json.len());
}

/// An ask kept before asks could say more reads back as it was.
#[test]
fn an_ask_kept_before_details_reads_back() {
    let ask: OwnerAsk = serde_json::from_str(
        r#"{"askId":"0123456789abcdef0123456789abcdef","question":"Ship it?","choices":[{"label":"Yes","consequence":"It ships."},{"label":"No","consequence":"It waits."}],"recommended":0,"reason":"It is ready.","askedAtMs":5}"#,
    )
    .unwrap();
    assert_eq!(ask, asked("0123456789abcdef0123456789abcdef", 5));
}

/// An ask is picked for its owner only when the agent allowed it and its
/// recommended choice is easy to undo, while it waits unpaused and the
/// owner's auto-pick is on; the pick's text names that choice the way an
/// answer does, and says why it was sent.
#[test]
fn auto_pick_is_due_only_for_an_allowed_easy_choice_and_names_it() {
    use crate::conversation::view::{DecisionSettings, OwnerDecision};
    let mut ask = asked("a", 10_000);
    let on = DecisionSettings {
        auto_pick: true,
        auto_pick_after_seconds: 300,
    };
    let decision = |ask: &OwnerAsk| OwnerDecision {
        ask: ask.clone(),
        outcome: None,
        auto_pick_paused: false,
        auto_pick_due_at_ms: None,
    };
    assert_eq!(decision(&ask).auto_pick_due(on), None);
    ask.auto_pick = Some(AutoPick::Allowed);
    ask.choices[0].undo = Some(AskUndo::Costly);
    assert_eq!(decision(&ask).auto_pick_due(on), None);
    ask.choices[0].undo = Some(AskUndo::Easy);
    assert_eq!(decision(&ask).auto_pick_due(on), Some(310_000));
    assert_eq!(
        decision(&ask).auto_pick_due(DecisionSettings {
            auto_pick: false,
            ..on
        }),
        None
    );
    let paused = OwnerDecision {
        auto_pick_paused: true,
        ..decision(&ask)
    };
    assert_eq!(paused.auto_pick_due(on), None);
    let answered = OwnerDecision {
        outcome: Some(OwnerDecisionOutcome::answered(&ask, "No", 20_000)),
        ..decision(&ask)
    };
    assert_eq!(answered.auto_pick_due(on), None);
    ask.auto_pick = Some(AutoPick::Never);
    assert_eq!(decision(&ask).auto_pick_due(on), None);

    let text = ask.auto_pick_text(300).unwrap();
    assert_eq!(
        text,
        "Yes. Picked for you on “Ship it?”: no answer in 5 minutes."
    );
    match OwnerDecisionOutcome::answered(&ask, &text, 1) {
        OwnerDecisionOutcome::Decided { choice, .. } => assert_eq!(choice, Some(0)),
        OwnerDecisionOutcome::Replaced { .. } => panic!("a pick is decided"),
    }
    assert_eq!(
        ask.auto_pick_text(3600).unwrap(),
        "Yes. Picked for you on “Ship it?”: no answer in 1 hour."
    );
    assert_eq!(
        ask.auto_pick_text(5400).unwrap(),
        "Yes. Picked for you on “Ship it?”: no answer in 90 minutes."
    );
}
