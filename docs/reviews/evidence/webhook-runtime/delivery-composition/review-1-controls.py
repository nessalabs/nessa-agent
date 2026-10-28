import hashlib,json,os,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[5]
delivery=Path('crates/nessa-server/src/webhooks/infrastructure/runtime/delivery.rs')
runtime=Path('crates/nessa-server/src/webhooks/infrastructure/runtime.rs')
paths=[delivery,runtime]
baseline={p:(root/p).read_bytes() for p in paths}
expected={'crates/nessa-server/src/webhooks/infrastructure/runtime/delivery.rs': 'd04c6b91286d934084d97d047b3baf2160e785b1b41687deab8d7ac7ac307a3b', 'crates/nessa-server/src/webhooks/infrastructure/runtime.rs': 'f2e6ec4678271fbc0f086a776c5fd76614bb146078ec352d06c7cae7bde5c15a'}
for p,data in baseline.items():
    assert hashlib.sha256(data).hexdigest()==expected[str(p)], "Use the recorded source checkpoint before running these controls"
backup=Path('/tmp/nessa-896-review1-final-control-baseline');backup.mkdir(exist_ok=True)
for p,data in baseline.items(): (backup/p.name).write_bytes(data)
rows=Path('/tmp/nessa-896-review1-final-controls.jsonl')
def replace_once(text,old,new):
    assert text.count(old)==1,(old,text.count(old))
    return text.replace(old,new,1)
def hold(text):
    return replace_once(text,'hold.cancel();\n            #[cfg(test)]\n            self.owner.dispatch_deadline.notify_one();','/* deliberately retain dispatch authority */\n            #[cfg(test)]\n            self.owner.dispatch_deadline.notify_one();')
def active(text):
    return replace_once(text,'''if !self
                    .owner
                    .store
                    .has_installed_intent(original.id, credential.clone())
                    .await
                    .map_err(store_error)?''','''if self.owner.store.registration(credential.clone()).await.map_err(store_error)?.is_none()''')
def fatal(text):
    a=text.index('        if let Some(Err(error)) = receipt {')
    b=text.index('        let expired =',a)
    return text[:a]+'''        if receipt.is_some_and(|result| result.is_err()) {
            return Err(CycleFailure::Fatal);
        }
'''+text[b:]
def idle(text):
    a=text.index('            match self\n                .observe_stop(&registration, ExpiryObserver::Source(registration.id))')
    b=text.index('            let conversations =',a)
    return text[:a]+text[b:]
def callback(text):
    a=text.index('        }; // The POST future is gone before authority can be released.')
    b=text.index('\n        *self',a)
    return text[:b]+'\n        let _premature_callback = self.owner.clock.unix_milliseconds();'+text[b:]
def scan(text):
    a=text.index('        // This worker is serial:')
    b=text.index('        let intents =',a)
    return text[:a]+text[b:]
probes=[
 ('dispatch_authority_retained_past_deadline',delivery,hold,'dispatch_deadline_releases_grant_while_native_sending_sql_stays_owned'),
 ('stopped_installation_mistaken_for_absence',runtime,active,'public_reissue_keeps_stopped_sending_history_separate_from_the_new_registration'),
 ('sending_io_seals_owner',delivery,fatal,'lost_sending_ack_reconciles_unknown_and_sends_next_record_in_the_same_live_owner'),
 ('idle_credential_stop_omitted',delivery,idle,'idle_expiry_retries_its_original_stop_after_publication_returns_without_grants'),
 ('clock_before_response_capture',delivery,callback,'actual_response_is_owned_before_a_later_clock_callback_panics'),
 ('live_unowned_sending_scan_omitted',delivery,scan,'public_reissue_keeps_stopped_sending_history_separate_from_the_new_registration'),
]
try:
    with rows.open('w') as results:
        for name,path,mutate,test in probes:
            for p,data in baseline.items(): (root/p).write_bytes(data)
            changed=mutate(baseline[path].decode());assert changed.encode()!=baseline[path]
            (root/path).write_text(changed)
            log=Path('/tmp/nessa-896-review1-final-control-'+name+'.log')
            print('RUN '+name+' -> '+test,flush=True)
            with log.open('w') as output:
                result=subprocess.run(['cargo','test','-p','nessa-server','--lib',test,'--','--nocapture'],cwd=root,stdout=output,stderr=subprocess.STDOUT)
            output=log.read_text()
            meaningful=result.returncode!=0 and 'test result: FAILED.' in output and test+' ... FAILED' in output and 'could not compile' not in output
            row=dict(control=name,test=test,exitCode=result.returncode,runtimeFailure=meaningful,log=str(log),source={str(p):hashlib.sha256(data).hexdigest() for p,data in baseline.items()})
            results.write(json.dumps(row)+'\n');results.flush()
            print('RESULT '+json.dumps(row),flush=True)
            assert meaningful,'Probe did not yield the required runtime assertion failure: '+name
finally:
    for p,data in baseline.items(): (root/p).write_bytes(data)
    assert all((root/p).read_bytes()==data for p,data in baseline.items())
    print('RESTORED ALL SOURCE BYTES',flush=True)
