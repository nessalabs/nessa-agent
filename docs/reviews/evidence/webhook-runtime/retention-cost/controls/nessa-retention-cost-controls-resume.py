from pathlib import Path
import json,shutil,subprocess,os,hashlib
root=Path('/workspace/nessa-webhook-retention');h=Path('/tmp/nessa-retention-cost-controls');deps=Path('/workspace/nessa-agent/target/debug/deps');snapshot=Path('/tmp/nessa-retention-dependency-snapshot');snapshot.mkdir(exist_ok=True)
for pattern in ['*.rlib','*.so']:
 for p in deps.glob(pattern):shutil.copy2(p,snapshot/p.name)
(h/'snapshot.json').write_text(json.dumps({'source':str(deps),'snapshot':str(snapshot),'files':len(list(snapshot.iterdir())),'bytes':sum(p.stat().st_size for p in snapshot.iterdir())},indent=2))
env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
a=json.loads(Path('/tmp/nessa-retention-oldhead-probes/auth-args.json').read_text());a=[v.replace(str(deps),str(snapshot)).replace('/tmp/nessa-retention-oldhead-probes/crates/nessa-auth/src/lib.rs',str(root/'crates/nessa-auth/src/lib.rs')).replace('/tmp/nessa-retention-oldhead-probes/libnessa_auth.rlib',str(h/'libnessa_auth.rlib')) for v in a]
(h/'auth-resumed-args.json').write_text(json.dumps(a,indent=2))
with (h/'auth-resumed-compile.txt').open('w') as out:rc=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
assert rc==0,'snapshot auth compile'
a=json.loads((h/'args.json').read_text());a=[v.replace(str(deps),str(snapshot)) for v in a];a=[v if not v.startswith('nessa_auth=') else f'nessa_auth={h}/libnessa_auth.rlib' for v in a];(h/'args-resumed.json').write_text(json.dumps(a,indent=2))
def compile_run(label,test):
 with (h/(label+'-compile.txt')).open('w') as out:rc=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
 assert rc==0,(label,'compile')
 with (h/(label+'-test.txt')).open('w') as out:return subprocess.run([str(h/'tests-bin'),test],env=env,stdout=out,stderr=subprocess.STDOUT).returncode
assert compile_run('snapshot-baseline','cleanup_tests')==0
journal=h/'crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs';original=journal.read_bytes();assert original==(root/'crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs').read_bytes()
controls=[('materialized-cutoff','cutoffs AS MATERIALIZED','cutoffs AS','terminal_cleanup_walks_one_indexed_cutoff'),('terminal-index','INDEXED BY terminal_deliveries','NOT INDEXED','terminal_cleanup_walks_one_indexed_cutoff'),('registration-index','INDEXED BY published_registration_audit','NOT INDEXED','registration_cleanup_does_not_visit')]
results=json.loads((h/'results.json').read_text())
for label,old,new,test in controls:
 try:
  assert old in original.decode();journal.write_text(original.decode().replace(old,new));caught=compile_run(label+'-snapshot-mutated',test)!=0
 finally:journal.write_bytes(original)
 restored=compile_run(label+'-snapshot-restored',test)==0
 results.append(dict(rule=label,caught=caught,restored=restored,byte_identical=journal.read_bytes()==(root/'crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs').read_bytes()))
 (h/'results.json').write_text(json.dumps(results,indent=2)+'\n');print(label,caught,restored,flush=True);assert caught and restored
assert compile_run('snapshot-final-restored','')==0
(h/'source-match.json').write_text(json.dumps({'journal_sha256':hashlib.sha256(journal.read_bytes()).hexdigest(),'schema_identical':(h/'crates/nessa-server/src/webhooks/infrastructure/outbox/schema.sql').read_bytes()==(root/'crates/nessa-server/src/webhooks/infrastructure/outbox/schema.sql').read_bytes(),'source_restored':True},indent=2))
