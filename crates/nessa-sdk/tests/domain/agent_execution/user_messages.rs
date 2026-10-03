use super::support::*;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};

fn image(seed: u8, size: u64) -> ImageReference {
    ImageReference::new(
        Sha256Digest::from_bytes([seed; 32]),
        ImageMediaType::Png,
        size,
    )
    .unwrap()
}

fn file(path: &str) -> LinkedFile {
    LinkedFile::new(path.into()).unwrap()
}

#[test]
fn an_image_reference_is_nonempty_and_within_the_single_image_budget() {
    let digest = Sha256Digest::from_bytes([7; 32]);
    assert_eq!(
        ImageReference::new(digest, ImageMediaType::Jpeg, 0),
        Err(ExecutionError::EmptyValue("image"))
    );
    assert_eq!(
        ImageReference::new(digest, ImageMediaType::Jpeg, ImageReference::MAX_BYTES + 1),
        Err(ExecutionError::ValueTooLong {
            field: "image",
            max_bytes: ImageReference::MAX_BYTES as usize,
        })
    );
    let largest = ImageReference::new(digest, ImageMediaType::Jpeg, ImageReference::MAX_BYTES);
    let largest = largest.unwrap();
    assert_eq!(largest.digest(), digest);
    assert_eq!(largest.media_type(), ImageMediaType::Jpeg);
    assert_eq!(largest.size(), ImageReference::MAX_BYTES);
}

#[test]
fn a_user_message_needs_text_or_an_image() {
    assert_eq!(
        UserMessage::new(None, Vec::new(), Vec::new()),
        Err(ExecutionError::EmptyValue("user message"))
    );

    let text = PromptText::new(" look \n").unwrap();
    let only_text = UserMessage::text_only(text.clone());
    assert_eq!(
        only_text,
        UserMessage::new(Some(text.clone()), Vec::new(), Vec::new()).unwrap()
    );
    assert_eq!(only_text.text_str(), " look \n");
    assert!(only_text.images().is_empty());

    let only_image = UserMessage::new(None, vec![image(1, 10)], Vec::new()).unwrap();
    assert_eq!(only_image.text(), None);
    assert_eq!(only_image.text_str(), "");
    assert_eq!(only_image.images(), [image(1, 10)]);
    assert!(only_image.files().is_empty());

    // A file alone is a message too: "here, look at this" is a whole turn.
    let only_file = UserMessage::new(None, Vec::new(), vec![file("/tmp/report.pdf")]).unwrap();
    assert_eq!(only_file.text(), None);
    assert!(only_file.images().is_empty());
    assert_eq!(only_file.files(), [file("/tmp/report.pdf")]);
}

#[test]
fn a_linked_file_is_an_absolute_path_that_can_be_said_in_a_link() {
    let file = LinkedFile::new("/Users/ada/notes/report final (v2).pdf".into()).unwrap();
    assert_eq!(file.path(), "/Users/ada/notes/report final (v2).pdf");
    // The name is derived, never carried beside the path, so the two cannot
    // disagree about which file is meant.
    assert_eq!(file.name(), "report final (v2).pdf");

    // A path is only ever what it was given: trimming it would name a
    // different file, because a file may be named with spaces at either end.
    let spaced = LinkedFile::new("/tmp/ report ".into()).unwrap();
    assert_eq!(spaced.name(), " report ");

    // A single root component is still a name, and a dotfile is a name.
    assert_eq!(LinkedFile::new("/passwd".into()).unwrap().name(), "passwd");
    assert_eq!(
        LinkedFile::new("/home/ada/.zshrc".into()).unwrap().name(),
        ".zshrc"
    );
    // A name that merely begins with a dot is an ordinary hidden file, and one
    // of three dots is an ordinary name.
    assert!(LinkedFile::new("/Users/ada/...".into()).is_ok());
}

#[test]
fn a_linked_file_refuses_every_path_it_could_not_carry_faithfully() {
    for (path, expected) in [
        ("", ExecutionError::EmptyValue("linked file path")),
        // Relative in every shape, including the ones that look absolute.
        ("report.pdf", ExecutionError::RelativeFilePath),
        ("./report.pdf", ExecutionError::RelativeFilePath),
        ("~/report.pdf", ExecutionError::RelativeFilePath),
        ("C:\\report.pdf", ExecutionError::RelativeFilePath),
        // A control character would end the link before the path did.
        ("/tmp/a\nb.pdf", ExecutionError::ControlCharacterInFilePath),
        ("/tmp/a\rb.pdf", ExecutionError::ControlCharacterInFilePath),
        ("/tmp/a\tb.pdf", ExecutionError::ControlCharacterInFilePath),
        (
            "/tmp/a\u{0}b.pdf",
            ExecutionError::ControlCharacterInFilePath,
        ),
        // C1 as well as C0, and the delete character. These are what
        // `char::is_control` covers and what the published path rule has to
        // cover with it, or a client lets through what the gateway refuses.
        (
            "/tmp/a\u{85}b.pdf",
            ExecutionError::ControlCharacterInFilePath,
        ),
        (
            "/tmp/a\u{9f}b.pdf",
            ExecutionError::ControlCharacterInFilePath,
        ),
        (
            "/tmp/a\u{7f}b.pdf",
            ExecutionError::ControlCharacterInFilePath,
        ),
        // Nothing at the end to call the file.
        ("/", ExecutionError::FilePathWithoutName),
        ("/tmp/", ExecutionError::FilePathWithoutName),
        // Components that do not survive being written as a URI: an empty one
        // is dropped, and a dot one is resolved away. Either would leave the
        // link naming a path nobody wrote.
        ("//tmp/a.pdf", ExecutionError::RepeatedSeparatorInFilePath),
        ("/tmp//a.pdf", ExecutionError::RepeatedSeparatorInFilePath),
        ("/tmp/.", ExecutionError::UnresolvedFilePathComponent),
        ("/tmp/..", ExecutionError::UnresolvedFilePathComponent),
        (
            "/tmp/../etc/passwd",
            ExecutionError::UnresolvedFilePathComponent,
        ),
        ("/tmp/./a.pdf", ExecutionError::UnresolvedFilePathComponent),
    ] {
        assert_eq!(LinkedFile::new(path.into()), Err(expected), "{path:?}");
    }

    // Quotes, parentheses, percent signs, and non-Latin names all survive:
    // none of them can open a link, and refusing them would refuse ordinary
    // files for no gain.
    for path in [
        "/tmp/it's a 100% \"good\" file (final).pdf",
        "/tmp/отчёт.pdf",
        "/tmp/a#b?c.pdf",
        // Brackets and backslashes are ordinary in a name. This value used to
        // refuse brackets because the ACP adapter writes the path into a
        // markdown link; that rule covered one of the three characters that
        // could close one, and is gone rather than extended a third time. The
        // adapter encodes both strings it interpolates, and
        // `prompt_link_attacks.rs` proves it over every scalar there is.
        "/tmp/[draft] notes.pdf",
        "/tmp/a]b.pdf",
        "/tmp/back\\slash.pdf",
        "/tmp/trailing\\",
    ] {
        assert_eq!(LinkedFile::new(path.into()).unwrap().path(), path);
    }

    // The length bound is over bytes, and it is the last thing a longer path
    // could have been.
    let longest = format!("/{}", "a".repeat(LinkedFile::MAX_PATH_BYTES - 1));
    assert_eq!(longest.len(), LinkedFile::MAX_PATH_BYTES);
    assert!(LinkedFile::new(longest.clone()).is_ok());
    assert_eq!(
        LinkedFile::new(format!("{longest}a")),
        Err(ExecutionError::ValueTooLong {
            field: "linked file path",
            max_bytes: LinkedFile::MAX_PATH_BYTES,
        })
    );

    // Bytes, and not characters. Every case above is ASCII, where the two
    // agree; a path of two-byte names is where they part, and it is the bound
    // the wire schema states and the one a filesystem actually imposes.
    let wide = format!("/{}a", "\u{451}".repeat(LinkedFile::MAX_PATH_BYTES / 2 - 1));
    assert_eq!(wide.len(), LinkedFile::MAX_PATH_BYTES);
    assert!(wide.chars().count() < LinkedFile::MAX_PATH_BYTES);
    assert!(LinkedFile::new(wide.clone()).is_ok());
    assert_eq!(
        LinkedFile::new(format!(
            "/{}",
            "\u{451}".repeat(LinkedFile::MAX_PATH_BYTES / 2)
        )),
        Err(ExecutionError::ValueTooLong {
            field: "linked file path",
            max_bytes: LinkedFile::MAX_PATH_BYTES,
        })
    );
}

#[test]
fn a_user_message_keeps_file_order_and_bounds_how_many_it_points_at() {
    let message = UserMessage::new(
        None,
        Vec::new(),
        vec![file("/b.txt"), file("/a.txt"), file("/b.txt")],
    )
    .unwrap();
    assert_eq!(
        message.files(),
        [file("/b.txt"), file("/a.txt"), file("/b.txt")]
    );
    assert_ne!(
        message,
        UserMessage::new(
            None,
            Vec::new(),
            vec![file("/a.txt"), file("/b.txt"), file("/b.txt")],
        )
        .unwrap()
    );

    let at_count = (0..UserMessage::MAX_FILES).map(|n| file(&format!("/{n}.txt")));
    assert!(UserMessage::new(None, Vec::new(), at_count.collect()).is_ok());
    let over_count = (0..=UserMessage::MAX_FILES).map(|n| file(&format!("/{n}.txt")));
    assert_eq!(
        UserMessage::new(None, Vec::new(), over_count.collect()),
        Err(ExecutionError::TooManyValues {
            field: "user message files",
            max: UserMessage::MAX_FILES,
        })
    );

    // No byte budget stands over files: nothing about them travels but the
    // paths, so the longest ten are still one ordinary message.
    let longest = format!("/{}", "a".repeat(LinkedFile::MAX_PATH_BYTES - 1));
    let full = vec![LinkedFile::new(longest).unwrap(); UserMessage::MAX_FILES];
    assert!(UserMessage::new(None, Vec::new(), full).is_ok());
}

#[test]
fn a_user_message_keeps_attachment_order_and_counts_repeats() {
    let message = UserMessage::new(
        None,
        vec![image(2, 5), image(1, 5), image(2, 5)],
        Vec::new(),
    )
    .unwrap();
    assert_eq!(message.images(), [image(2, 5), image(1, 5), image(2, 5)]);
    // Order is part of what was said, so it is part of equality.
    assert_ne!(
        message,
        UserMessage::new(
            None,
            vec![image(1, 5), image(2, 5), image(2, 5)],
            Vec::new()
        )
        .unwrap()
    );
}

#[test]
fn a_user_message_bounds_image_count_and_total_bytes() {
    let at_count = (0..UserMessage::MAX_IMAGES as u8).map(|seed| image(seed, 1));
    assert!(UserMessage::new(None, at_count.collect(), Vec::new()).is_ok());
    let over_count = (0..=UserMessage::MAX_IMAGES as u8).map(|seed| image(seed, 1));
    assert_eq!(
        UserMessage::new(None, over_count.collect(), Vec::new()),
        Err(ExecutionError::TooManyValues {
            field: "user message images",
            max: UserMessage::MAX_IMAGES,
        })
    );

    // Two of the largest image fill the total exactly; one more byte does not fit.
    let full = vec![image(9, ImageReference::MAX_BYTES); 2];
    assert!(UserMessage::new(None, full.clone(), Vec::new()).is_ok());
    let mut over = full;
    over.push(image(9, 1));
    assert_eq!(
        UserMessage::new(None, over, Vec::new()),
        Err(ExecutionError::ValueTooLong {
            field: "user message images",
            max_bytes: UserMessage::MAX_IMAGE_BYTES as usize,
        })
    );
}

fn app(tool_id: &str) -> McpAppSource {
    McpAppSource::new(
        ExecutionId::new("turn-1").unwrap(),
        ToolCallId::new(tool_id).unwrap(),
        McpTool::new("charts", "plot").unwrap(),
    )
    .unwrap()
}

#[test]
fn an_apps_tool_call_identity_is_bounded_as_an_executions_is() {
    let long = "t".repeat(McpAppSource::MAX_TOOL_ID_BYTES + 1);
    assert_eq!(
        McpAppSource::new(
            ExecutionId::new("turn-1").unwrap(),
            ToolCallId::new(long).unwrap(),
            McpTool::new("charts", "plot").unwrap(),
        ),
        Err(ExecutionError::ValueTooLong {
            field: "app tool call ID",
            max_bytes: McpAppSource::MAX_TOOL_ID_BYTES,
        })
    );
    let exact = "t".repeat(McpAppSource::MAX_TOOL_ID_BYTES);
    assert_eq!(app(&exact).tool_id().as_str(), exact);
}

#[test]
fn an_apps_context_holds_text_or_one_json_object_within_its_bound() {
    assert_eq!(
        AppModelContext::new(app("call-1"), "update-1", None, None),
        Ok(None)
    );
    // An empty text is none, so it alone is no context.
    assert_eq!(
        AppModelContext::new(app("call-1"), "update-1", Some(String::new()), None),
        Ok(None)
    );
    let both = AppModelContext::new(
        app("call-1"),
        "update-1",
        Some("May".into()),
        Some(r#" {"month":5}"#.into()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(both.text(), Some("May"));
    assert_eq!(both.structured_content(), Some(r#" {"month":5}"#));
    for not_an_object in ["[1]", "5", "\"x\"", "{", "{} {}", "", "null"] {
        assert_eq!(
            AppModelContext::new(app("call-1"), "update-1", None, Some(not_an_object.into())),
            Err(ExecutionError::InvalidStructuredContent),
            "{not_an_object:?}"
        );
    }
    // The bound is on both parts together, in UTF-8 bytes: exactly at it is
    // a context, one byte past it is not, multibyte text included.
    let half = "é".repeat(AppModelContext::MAX_BYTES / 4);
    let object = format!(
        r#"{{"k":"{}"}}"#,
        "x".repeat(AppModelContext::MAX_BYTES / 2 - 8)
    );
    assert_eq!(half.len() + object.len(), AppModelContext::MAX_BYTES);
    assert!(AppModelContext::new(
        app("call-1"),
        "update-1",
        Some(half.clone()),
        Some(object.clone())
    )
    .is_ok());
    assert_eq!(
        AppModelContext::new(
            app("call-1"),
            "update-1",
            Some(format!("{half}x")),
            Some(object)
        ),
        Err(ExecutionError::ValueTooLong {
            field: "app context",
            max_bytes: AppModelContext::MAX_BYTES,
        })
    );
}

#[test]
fn a_message_is_the_persons_until_said_otherwise_and_carries_at_most_four_contexts() {
    let message = UserMessage::text_only(PromptText::new("hello").unwrap());
    assert_eq!(message.sender(), &MessageSender::Person);
    assert!(message.app_model_context().is_empty());

    let sent = message.clone().sent_by(MessageSender::App(app("call-1")));
    assert_eq!(sent.sender(), &MessageSender::App(app("call-1")));
    // Who wrote it is part of the message: the same words from an app are
    // another message, so a retry that changed it is a conflict.
    assert_ne!(sent, message);

    let context = |id: &str| {
        AppModelContext::new(app(id), "update-1", Some("ctx".into()), None)
            .unwrap()
            .unwrap()
    };
    let four: Vec<_> = ["a", "b", "c", "d"].into_iter().map(context).collect();
    let carrying = message
        .clone()
        .with_app_model_context(four.clone())
        .unwrap();
    assert_eq!(carrying.app_model_context(), four.as_slice());
    assert_ne!(carrying, message);
    let mut five = four;
    five.push(context("e"));
    assert_eq!(
        message.with_app_model_context(five),
        Err(ExecutionError::TooManyValues {
            field: "user message app contexts",
            max: UserMessage::MAX_APP_MODEL_CONTEXTS,
        })
    );
}

#[test]
fn an_apps_context_names_the_update_that_gave_it_by_a_bounded_identity() {
    let context =
        |update: String| AppModelContext::new(app("call-1"), update, Some("ctx".into()), None);
    assert_eq!(
        context(" ".into()),
        Err(ExecutionError::EmptyValue("app context update"))
    );
    assert_eq!(
        context("u".repeat(AppModelContext::MAX_UPDATE_BYTES + 1)),
        Err(ExecutionError::ValueTooLong {
            field: "app context update",
            max_bytes: AppModelContext::MAX_UPDATE_BYTES,
        })
    );
    let exact = "u".repeat(AppModelContext::MAX_UPDATE_BYTES);
    assert_eq!(context(exact.clone()).unwrap().unwrap().update(), exact);
    // Which update gave it is part of it: the same words from another update
    // are another context.
    assert_ne!(context("a".into()), context("b".into()));
}

#[test]
fn an_app_and_its_context_count_every_byte_they_hold() {
    let source = app("call-1");
    assert_eq!(source.execution_id().as_str(), "turn-1");
    assert_eq!(source.tool_id().as_str(), "call-1");
    assert_eq!(
        (source.tool().server(), source.tool().tool()),
        ("charts", "plot")
    );
    assert_eq!(
        source.payload_bytes(),
        "turn-1".len() + "call-1".len() + "charts".len() + "plot".len()
    );
    let context = AppModelContext::new(
        source.clone(),
        "update-1",
        Some("May".into()),
        Some(r#"{"m":5}"#.into()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(context.app(), &source);
    assert_eq!(
        context.payload_bytes(),
        source.payload_bytes() + "update-1".len() + "May".len() + r#"{"m":5}"#.len()
    );
    let structure_only = AppModelContext::new(source.clone(), "update-2", None, Some("{}".into()))
        .unwrap()
        .unwrap();
    assert_eq!(structure_only.text(), None);
    assert_eq!(
        structure_only.payload_bytes(),
        source.payload_bytes() + "update-2".len() + 2
    );
    let text_only = AppModelContext::new(source, "update-3", Some("x".into()), None)
        .unwrap()
        .unwrap();
    assert_eq!(text_only.structured_content(), None);
    assert_eq!(MessageSender::default(), MessageSender::Person);
}
