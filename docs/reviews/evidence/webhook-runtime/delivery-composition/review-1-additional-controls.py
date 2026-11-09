import hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[5]
admission=Path('crates/nessa-server/src/webhooks/application/admission.rs')
runtime=Path('crates/nessa-server/src/webhooks/infrastructure/runtime.rs')
files=[admission,runtime]
baseline={p:(root/p).read_bytes() for p in files}
expected={'crates/nessa-server/src/webhooks/application/admission.rs': 'cb665a921eec97877b1825b8cc56f6a05e4d9526fa198561b2daa1fc07a134c5', 'crates/nessa-server/src/webhooks/infrastructure/runtime.rs': 'f2e6ec4678271fbc0f086a776c5fd76614bb146078ec352d06c7cae7bde5c15a'}
for p,data in baseline.items():
    assert hashlib.sha256(data).hexdigest()==expected[str(p)], "Use the recorded source checkpoint before running these controls"
backup=Path('/tmp/nessa-896-review1-new-control-baseline');backup.mkdir(exist_ok=True)
for p,data in baseline.items(): (backup/p.name).write_bytes(data)
def partial(text):
    old='(state.credential.take(), state.grant.take())'
    assert text.count(old)==1
    return text.replace(old,'(None::<AccessHold>, None::<RevisionHold>)',1)
def route(text):
    old='''        // Retained history selects the command route even after seed removal.'''
    assert text.count(old)==1
    return text.replace(old,'''        if !self.owner.configurations.contains_key(&request.principal.id) {
            return self.owner.base.issue(request).await;
        }
'''+old,1)
probes=[('partial_dispatch_fences_retained',admission,partial,'dispatch_deadline_releases_partial_admission_before_joining_native_metadata'),('seed_removal_bypasses_retained_route',runtime,route,'cold_runtime_without_seeds_replays_webhook_history_before_ordinary_routing')]
try:
    with Path('/tmp/nessa-896-review1-new-controls.jsonl').open('w') as results:
        for name,path,mutate,test in probes:
            for p,data in baseline.items(): (root/p).write_bytes(data)
            (root/path).write_text(mutate(baseline[path].decode()))
            log=Path('/tmp/nessa-896-review1-control-'+name+'.log')
            print('RUN '+name,flush=True)
            with log.open('w') as output:
                result=subprocess.run(['cargo','test','-p','nessa-server','--lib',test,'--','--nocapture'],cwd=root,stdout=output,stderr=subprocess.STDOUT)
            text=log.read_text()
            meaningful=result.returncode!=0 and 'test result: FAILED.' in text and test+' ... FAILED' in text and 'could not compile' not in text
            row=dict(control=name,test=test,exitCode=result.returncode,runtimeFailure=meaningful,log=str(log),source={str(p):hashlib.sha256(data).hexdigest() for p,data in baseline.items()})
            results.write(json.dumps(row)+'\n');results.flush();print('RESULT '+json.dumps(row),flush=True)
            assert meaningful,'Expected named runtime failure: '+name
finally:
    for p,data in baseline.items(): (root/p).write_bytes(data)
    assert all((root/p).read_bytes()==data for p,data in baseline.items())
    print('RESTORED ALL SOURCE BYTES',flush=True)
