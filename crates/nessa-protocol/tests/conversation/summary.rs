//! A conversation's summary rules, tested without storage, a runtime, or an agent.
use super::{ConversationPreview, ConversationSummary, ConversationTitle};

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
