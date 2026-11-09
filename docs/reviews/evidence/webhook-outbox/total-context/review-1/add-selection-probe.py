from pathlib import Path
p=Path('/tmp/independent-total-context-review/crates/nessa-server/tests/webhooks/infrastructure/outbox.rs')
p.write_text(p.read_text()+r'''
#[test]
fn independent_owed_selection_scans_unrelated_published_rows() {
    use nessa_local_database::rusqlite::StatementStatus;
    let source=include_str!("/workspace/nessa-agent/crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs");
    let line=source.lines().find(|l|l.contains("SELECT id,registration,delivery,record,published FROM audit WHERE (?1=0 OR published=0)")).unwrap();
    let query=line.split("prepare(\"").nth(1).unwrap().split("\")").next().unwrap();
    for count in [0,100,10000] {
        let mut c=Connection::open_in_memory().unwrap();c.execute_batch(SCHEMA).unwrap();
        let tx=c.transaction().unwrap();
        // These rows exercise the SQL selection only, not typed restoration.
        for i in 0..=count {tx.execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,'synthetic','{}',?2)",params![i.to_string(),if i==count {0}else{1}]).unwrap();}
        tx.commit().unwrap();
        let mut q=c.prepare(query).unwrap();
        let rows=q.query_map([true],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();assert_eq!(rows.len(),1);
        let scan=q.get_status(StatementStatus::FullscanStep);let steps=q.get_status(StatementStatus::VmStep);
        let mut indexed=c.prepare("SELECT id,registration,delivery,record,published FROM audit WHERE published=0").unwrap();
        let rows=indexed.query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();assert_eq!(rows.len(),1);
        eprintln!("published={count} actual_fullscan={scan} actual_vm={steps} indexed_fullscan={} indexed_vm={}",indexed.get_status(StatementStatus::FullscanStep),indexed.get_status(StatementStatus::VmStep));
        if count==10000 {assert_eq!(scan,10000);assert_eq!(indexed.get_status(StatementStatus::FullscanStep),0);}
    }
}
''')
