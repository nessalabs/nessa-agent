//! A real semantic corpus uses the public portable core in separate native processes.
use super::*;

#[test]
fn online_semantic_corpus_converges_after_receiver_restart_and_live_suffix() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let mut gateway = Gateway::start_semantic(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let oracle = |name: &str| -> Value {
        serde_json::from_slice(&std::fs::read(root.join(format!("semantic-{name}.json"))).unwrap())
            .unwrap()
    };
    let sync = |cache: &Path| {
        let (ok, report) = command(record_command(&root, cache, &setup, "100"), false);
        assert!(ok, "{report:?}");
        let report = report.unwrap();
        assert_eq!(report["work"]["complete"], true, "{report}");
        assert_eq!(report["durable"]["status"], "complete", "{report}");
        report
    };
    let show = |cache: &Path| {
        let (ok, report) = command(
            vec![
                "show".into(),
                cache.to_string_lossy().into_owned(),
                setup.receiver.clone(),
                setup.gateway.clone(),
                setup.conversation.clone(),
            ],
            false,
        );
        assert!(ok, "{report:?}");
        report.unwrap()
    };
    let without_revision = |mut view: Value| {
        assert!(view["revision"].take().is_string(), "{view}");
        view
    };
    let assert_matches = |report: &Value, saved: &Value, expected: &Value| {
        // Captured source evidence and confirmed receiver accounting have
        // different owners; compare both instead of treating the head as A.
        assert_eq!(report["capturedCheck"]["head"], expected["capturedHead"]);
        for field in ["applied", "downloaded", "facts"] {
            assert_eq!(
                report["durable"]["progress"][field],
                expected["progress"][field]
            );
            assert_eq!(saved[field], expected["progress"][field]);
        }
        assert_eq!(
            without_revision(saved["view"].clone()),
            without_revision(expected["view"].clone())
        );
    };
    let prefix = oracle("prefix");
    let prefix_report = sync(&cache);
    let prefix_view = show(&cache); // New process, reopening the durable checkpoint.
    assert_matches(&prefix_report, &prefix_view, &prefix);
    assert_eq!(prefix_view["view"]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        prefix_view["view"]["messages"][0]["userText"],
        "First accepted input"
    );
    assert_eq!(
        prefix_view["view"]["messages"][1]["userText"],
        "Second queued input"
    );
    assert_eq!(prefix_view["view"]["permissions"], json!([]));
    assert!(prefix_view["view"]["messages"][0]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|part| part["text"] == "Committed before permission."));

    let mut receiver = WatchChild::spawn(vec![
        "watch".into(),
        profile_for(&root, &cache),
        setup.conversation.clone(),
        "100".into(),
        "2".into(),
    ]);
    let watch = watch_registered(&mut receiver);
    let prefix_facts = prefix["progress"]["facts"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (rechecked, _) = watch_pass_report(&mut receiver, "recheck", prefix_facts);
    assert_matches(&rechecked, &show(&cache), &prefix);
    // The public hint pass waits until all terminal producer writes are confirmed.
    gateway.act("hold");
    gateway.act("settle");
    let terminal = oracle("terminal");
    watch_hint(&mut receiver, &watch, false);
    gateway.act("release");
    let terminal_facts = terminal["progress"]["facts"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (resumed, _) = watch_pass_report(&mut receiver, "hint", terminal_facts);
    assert!(watch_end(receiver, "passesExhausted"));
    let resumed_view = show(&cache);
    assert_matches(&resumed, &resumed_view, &terminal);
    assert!(resumed_view["view"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["status"] == "completed"));
    let text = |message: &Value| {
        message["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part["kind"] == "text")
            .map(|part| part["text"].as_str().unwrap())
            .collect::<String>()
    };
    assert_eq!(
        text(&resumed_view["view"]["messages"][0]),
        "Committed before permission.Response: First accepted input"
    );
    assert_eq!(
        text(&resumed_view["view"]["messages"][1]),
        "Response: Second queued input"
    );
    assert!(
        terminal["progress"]["facts"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > prefix["progress"]["facts"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );

    let fresh_directory = tempfile::tempdir().unwrap();
    let fresh = setup_cache(fresh_directory.path());
    let replay = sync(&fresh);
    let replay_view = show(&fresh);
    assert_matches(&replay, &replay_view, &terminal);
    assert_eq!(
        without_revision(replay_view["view"].clone()),
        without_revision(resumed_view["view"].clone())
    );
    // Repeated pulls and render-only commands cannot dispatch the copied input.
    assert_matches(&sync(&cache), &show(&cache), &terminal);
    gateway.act("verify");

    let catalogue = |cache: &Path| {
        let (ok, report) = command(
            vec![
                "sync-catalogue".into(),
                profile_for(&root, cache),
                "10".into(),
            ],
            false,
        );
        assert!(ok, "{report:?}");
        assert_eq!(report.unwrap()["work"]["complete"], true);
    };
    catalogue(&cache);
    gateway.act("delete");
    catalogue(&cache);
    let deleted = show(&cache);
    assert_eq!(deleted["state"], "deleted", "{deleted}");
    assert!(deleted["view"].is_null(), "{deleted}");
    // Restart the gateway and receiver: a saved semantic continuation cannot
    // override the separately owned deletion metadata.
    drop(gateway);
    let _restarted = Gateway::start(&root);
    catalogue(&cache);
    assert_eq!(show(&cache)["state"], "deleted");
}
