import json,hashlib,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-retained-progress-independent-review');a=json.loads((p/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
root=p/'crates/nessa-server/src/webhooks/infrastructure/outbox'
cases=[
 ('shared-owner',root/'restoration.rs','        progress::validate(connection, registration)?;','        let _ = registration;','initial_rejection_with_only_retained_source_history'),
 ('initial-identity',root/'restoration/progress.rs','                .is_some()','                .is_some() && false','published_initial_identity_excludes_a_second_pending_identity'),
 ('scoped-summary',root/'restoration/progress.rs','FROM audit WHERE registration=?1','FROM audit WHERE ?1 IS NOT NULL','independent_reset_and_sending_proof_is_scoped'),
 ('schema-inventory',root/'schema.sql','PRAGMA user_version = 1;','PRAGMA user_version = 1;\nCREATE TABLE unclassified_progress(id TEXT);','retained_progress_inventory_covers_the_canonical_tables'),
]
def run(label,test):
 with(p/(label+'-compile.txt')).open('w')as f:subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 with(p/(label+'-tests.txt')).open('w')as f:return subprocess.run([str(p/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
results=[]
for name,file,before,after,test in cases:
 original=file.read_bytes();s=original.decode();assert s.count(before)==1,(name,s.count(before))
 try:
  file.write_text(s.replace(before,after));assert file.read_bytes()!=original
  (p/(name+'-edit.json')).write_text(json.dumps(dict(path=str(file),before=before,after=after,sourceSHA256=hashlib.sha256(original).hexdigest(),mutatedSHA256=hashlib.sha256(file.read_bytes()).hexdigest()),indent=2)+'\n')
  assert run(name+'-broken',test)!=0,(name,'not caught')
 finally:file.write_bytes(original);assert file.read_bytes()==original
 assert run(name+'-restored',test)==0,(name,'restored failure')
 results.append(dict(rule=name,caught=True,restored=True));(p/'controls.json').write_text(json.dumps(results,indent=2)+'\n');print(name,flush=True)
