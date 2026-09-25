//! The local summary store: private files replaced whole, read back only when
//! they hold what the summary rules could have produced.
use crate::conversation::{
    application::{ConversationError, ConversationSummaries},
    domain::{ConversationId, ConversationSummary, LATEST_TIME_MS},
    infrastructure::LocalConversationSummaries,
};
use nessa_local_storage::{self as storage, OpenMode};
use std::io::Write;
use uuid::Uuid;

fn id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}

#[tokio::test]
async fn a_summary_is_replaced_whole_and_survives_reopening() {
    let root = std::env::temp_dir().join(format!("nessa-summary-test-{}", Uuid::new_v4()));
    let summaries = LocalConversationSummaries::new(root.clone()).unwrap();
    let conversation = id();
    assert_eq!(summaries.load(&conversation).await.unwrap(), None);

    let first = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    summaries
        .record(&conversation, first.clone())
        .await
        .unwrap();
    assert_eq!(
        summaries.load(&conversation).await.unwrap(),
        Some(first.clone())
    );
    let replied = ConversationSummary::after_reply(Some(&first), &"é".repeat(400), 20).unwrap();
    summaries
        .record(&conversation, replied.clone())
        .await
        .unwrap();

    // An interrupted write leaves a temporary the next owner releases.
    let unfinished = root.join(format!(".nessa-{}.tmp", "a".repeat(32)));
    storage::open(&unfinished, OpenMode::CreateNew)
        .unwrap()
        .write_all(b"{")
        .unwrap();
    drop(summaries);
    let summaries = LocalConversationSummaries::new(root.clone()).unwrap();
    assert!(!unfinished.exists());
    assert_eq!(summaries.load(&conversation).await.unwrap(), Some(replied));
    // One file per conversation, whatever was written to it.
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_summary_that_is_not_what_the_rules_produce_is_refused() {
    let root = std::env::temp_dir().join(format!("nessa-summary-test-{}", Uuid::new_v4()));
    let summaries = LocalConversationSummaries::new(root.clone()).unwrap();
    let conversation = id();
    let other = id();
    let path = root.join(format!("{conversation}.json"));
    for bytes in [
        b"{".to_vec(),
        // Another conversation's summary under this one's name.
        format!(r#"{{"id":"{other}","title":null,"preview":null,"updated_at_ms":1,"archived":false}}"#).into_bytes(),
        // A preview that is not one line.
        format!(r#"{{"id":"{conversation}","title":null,"preview":"a\nb","updated_at_ms":1,"archived":false}}"#)
            .into_bytes(),
        // A title longer than any first message titles a conversation.
        format!(
            r#"{{"id":"{conversation}","title":"{}","preview":null,"updated_at_ms":1,"archived":false}}"#,
            "a".repeat(49)
        )
        .into_bytes(),
        format!(
            r#"{{"id":"{conversation}","title":null,"preview":null,"updated_at_ms":1,"archived":false,"extra":1}}"#
        )
        .into_bytes(),
        // Silent about whether it was archived, which is a person's decision
        // and never assumed.
        format!(r#"{{"id":"{conversation}","title":null,"preview":null,"updated_at_ms":1}}"#)
            .into_bytes(),
        // Past the read bound.
        format!(
            r#"{{"id":"{conversation}","title":null,"preview":null,"updated_at_ms":1,"archived":false}}{}"#,
            " ".repeat(4096)
        )
        .into_bytes(),
    ] {
        let _ = std::fs::remove_file(&path);
        storage::open(&path, OpenMode::CreateNew)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert!(
            matches!(
                summaries.load(&conversation).await,
                Err(ConversationError::Metadata)
            ),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }
    // A write replaces it with a summary that reads back.
    let summary = ConversationSummary::after_message(None, "fresh", None, 5);
    summaries
        .record(&conversation, summary.clone())
        .await
        .unwrap();
    assert_eq!(summaries.load(&conversation).await.unwrap(), Some(summary));

    // A directory that is gone refuses writes rather than creating one.
    std::fs::remove_dir_all(&root).unwrap();
    assert!(matches!(
        summaries
            .record(
                &conversation,
                ConversationSummary::after_message(None, "x", None, 6)
            )
            .await,
        Err(ConversationError::Metadata)
    ));
}

#[tokio::test]
async fn an_archived_summary_reads_back_archived_and_an_erased_one_is_gone() {
    let root = std::env::temp_dir().join(format!("nessa-summary-test-{}", Uuid::new_v4()));
    let summaries = LocalConversationSummaries::new(root.clone()).unwrap();
    let conversation = id();
    let kept = id();
    let said = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    let archived = said.after_archiving(true);
    summaries
        .record(&conversation, archived.clone())
        .await
        .unwrap();
    summaries.record(&kept, said.clone()).await.unwrap();
    drop(summaries);
    let summaries = LocalConversationSummaries::new(root.clone()).unwrap();
    assert_eq!(summaries.load(&conversation).await.unwrap(), Some(archived));

    summaries.erase(&conversation).await.unwrap();
    assert_eq!(summaries.load(&conversation).await.unwrap(), None);
    assert!(!root.join(format!("{conversation}.json")).exists());
    // Erasing what is gone is not an error, and nothing else is touched.
    summaries.erase(&conversation).await.unwrap();
    assert_eq!(summaries.load(&kept).await.unwrap(), Some(said));

    // A directory that is gone refuses the erase rather than reporting it done.
    std::fs::remove_dir_all(&root).unwrap();
    assert!(matches!(
        summaries.erase(&kept).await,
        Err(ConversationError::Metadata)
    ));
}

#[tokio::test]
async fn a_summary_dated_past_the_latest_time_is_unreadable() {
    let root = std::env::temp_dir().join(format!("nessa-summary-test-{}", Uuid::new_v4()));
    let summaries = LocalConversationSummaries::new(root.clone()).unwrap();
    let write = |conversation: &ConversationId, at: u64| {
        storage::open(&root.join(format!("{conversation}.json")), OpenMode::CreateNew)
            .unwrap()
            .write_all(
                format!(
                    r#"{{"id":"{conversation}","title":"Plan","preview":"Plan","updated_at_ms":{at},"archived":false}}"#
                )
                .as_bytes(),
            )
            .unwrap();
    };
    // A hand-edited or damaged file with a time no reader of a list can
    // carry: unreadable, as any damaged summary is — its row goes bare, the
    // list does not fail.
    let damaged = id();
    write(&damaged, LATEST_TIME_MS + 1);
    assert!(matches!(
        summaries.load(&damaged).await,
        Err(ConversationError::Metadata)
    ));
    // The latest time itself is read.
    let latest = id();
    write(&latest, LATEST_TIME_MS);
    assert_eq!(
        summaries
            .load(&latest)
            .await
            .unwrap()
            .unwrap()
            .updated_at_ms(),
        LATEST_TIME_MS
    );
    // And no summary built here can hold a later one.
    let said = ConversationSummary::after_message(None, "Plan", None, u64::MAX);
    assert_eq!(said.updated_at_ms(), LATEST_TIME_MS);
    std::fs::remove_dir_all(root).ok();
}
