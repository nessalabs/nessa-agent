
#[test]
fn independent_competing_receipts_deliver_one_secret_and_keep_purpose_identity() {
    for different_purpose in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = Arc::new(open_store(&path).unwrap());
        store.bootstrap(bootstrap()).unwrap();
        let gate = Arc::new(std::sync::Barrier::new(3));
        let one = request();
        let mut two = one.clone(); two.credential_id = "competing-proposal".into(); two.issued_at += 1;
        if different_purpose { two.purpose = CredentialIssuePurpose::Product; }
        let workers = [one, two].into_iter().map(|command| {
            let store = store.clone(); let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                match store.issue_sync(command.clone()) {
                    Ok(IssueCredentialOutcome::Issued { metadata, transitions, .. }) => ("issued", Some((metadata, transitions)), command),
                    Ok(IssueCredentialOutcome::ExistingSecretUnavailable { metadata, transitions }) => ("existing", Some((metadata, transitions)), command),
                    Err(LocalStoreError::Conflict) => ("conflict", None, command),
                    Err(LocalStoreError::Locked) => ("busy", None, command),
                    _ => panic!("unexpected competing outcome"),
                }
            })
        }).collect::<Vec<_>>();
        gate.wait();
        let results = workers.into_iter().map(|w| w.join().unwrap()).map(|(kind, receipt, command)| {
            if kind != "busy" { return (kind, receipt); }
            // The existing synchronous API refuses contention, rather than
            // waiting. After both physical calls settle, retry the exact command.
            match store.issue_sync(command) {
                Ok(IssueCredentialOutcome::ExistingSecretUnavailable { metadata, transitions }) => ("existing", Some((metadata, transitions))),
                Err(LocalStoreError::Conflict) => ("conflict", None),
                _ => panic!("bounded retry after settled contention"),
            }
        }).collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|(kind, _)| *kind == "issued").count(), 1);
        assert_eq!(results.iter().filter(|(kind, _)| *kind == if different_purpose { "conflict" } else { "existing" }).count(), 1);
        if !different_purpose { assert_eq!(results[0].1, results[1].1); }
        assert_eq!(store.list_sync(&ListCredentialsRequest { organization_id: "org-1".into() }).unwrap().len(), 2);
        assert_eq!(store.list_transitions_sync(&ListTransitionsRequest { organization_id: "org-1".into() }).unwrap().len(), 2);
    }
}
