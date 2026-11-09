from pathlib import Path
import json,subprocess,os
probe=Path('/tmp/nessa-webhook-outbox-probes')
p=probe/'crates/nessa-server/tests/webhooks/infrastructure/outbox.rs'
s=p.read_text();p.write_text(s+r'''
#[tokio::test]
async fn demonstration_owed_delivery_history_is_not_admitted_live() {
    let f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let id=edit_evidence(&f,"webhook_delivery","source_checkpoint",|r| {
        r["beforeState"]=r["afterState"].clone();
        r["before"]=json!("queued");
        r["afterState"]["phase"]=json!("not_sent");
        r["after"]=json!("not_sent");
        r["cause"]=json!("admission_refused");
        r["initiator"]=json!(format!("delivery:{}",r["deliveryId"].as_str().unwrap()));
    });
    let c=f.store.owner.connection.lock().unwrap();
    let text:String=c.query_row("SELECT record FROM audit WHERE id=?1",[&id],|r|r.get(0)).unwrap();
    audit::Record::decode(&text).unwrap();
    drop(c);
    // Demonstrates the unwanted acceptance at ef015c87, not a fixed regression.
    assert_eq!(f.store.due(3000).await.unwrap().len(),1);
    assert!(f.audit.join(format!("{id}.json")).exists());
}
''')
args=json.loads((probe/'args.json').read_text());args[args.index('-o')+1]=str(probe/'history-probe-bin');env=dict(os.environ,RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
try:
 c=subprocess.run(args,env=env,text=True,capture_output=True);assert c.returncode==0,c.stderr
 r=subprocess.run([str(probe/'history-probe-bin'),'demonstration_owed_delivery_history'],env=env,text=True,capture_output=True);print(r.stdout+r.stderr);raise SystemExit(r.returncode)
finally:p.write_text(s)
