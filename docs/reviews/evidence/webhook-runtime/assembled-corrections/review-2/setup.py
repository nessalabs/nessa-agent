import os,json,shutil,subprocess,hashlib
from pathlib import Path
r=Path('/workspace/nessa-webhook-qualified');o=Path('/tmp/nessa-assembled-final-review')
for rel in ['crates/nessa-auth','crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:
 shutil.copytree(r/rel,o/rel,dirs_exist_ok=True)
s=Path('/tmp/nessa-retention-independent-review/lib.rs').read_text().replace('/tmp/nessa-retention-independent-review',str(o));(o/'lib.rs').write_text(s)
for name in ['auth-resumed-args','args-resumed']:
 a=json.loads((Path('/tmp/nessa-retention-independent-review')/(name+'.json')).read_text())
 a=[v.replace('/workspace/nessa-webhook-retention',str(o)).replace('/tmp/nessa-retention-independent-review',str(o)) for v in a]
 (o/(name+'.json')).write_text(json.dumps(a,indent=2))
proof={str(p.relative_to(r)):hashlib.sha256(p.read_bytes()).hexdigest() for rel in ['crates/nessa-auth','crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks'] for p in (r/rel).rglob('*') if p.is_file()}
assert all(hashlib.sha256((o/rel).read_bytes()).hexdigest()==h for rel,h in proof.items())
(o/'provenance.json').write_text(json.dumps(proof,indent=2))
env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
for name in ['auth-resumed-args','args-resumed']:
 with (o/(name+'-compile.txt')).open('w') as f:subprocess.run(json.loads((o/(name+'.json')).read_text()),stdout=f,stderr=subprocess.STDOUT,env=env,check=True)
with (o/'baseline.txt').open('w') as f:subprocess.run([str(o/'tests-bin'),'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env,check=True)
print((o/'baseline.txt').read_text()[-400:])
