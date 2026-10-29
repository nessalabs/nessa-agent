import hashlib,json,os,subprocess
from pathlib import Path
r=Path('/tmp/nessa-lifecycle-frozen');out=Path('/tmp/nessa-w24-M2-controls');out.mkdir(exist_ok=True)
a=json.loads((r/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
p=r/'crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs'
cases=[('owed-issue', 'audit::Cause::CredentialIssue => {\n                    if owed','audit::Cause::CredentialIssue => {\n                    if false && owed','completed_issue_cannot_be_owed'),('phase-proof','!matches!(&live, Some(live) if live.lifecycle.phase() == RegistrationPhase::Active)','false','completed_issue_cannot_be_owed'),('cursor-proof','SELECT EXISTS(SELECT 1 FROM cursors WHERE registration=?1)','SELECT 0','completed_issue_cannot_be_owed'),('delivery-proof','EXISTS(SELECT 1 FROM deliveries WHERE registration=?1)','0','completed_issue_cannot_be_owed'),('published-history','audit::Cause::CredentialIssue => {\n                    if owed','audit::Cause::CredentialIssue => {\n                    if true','published_original_issue'),('cursor-index','FROM cursors WHERE registration=?1)','FROM cursors NOT INDEXED WHERE registration=?1)','issuance_work_proof'),('delivery-index','FROM deliveries WHERE registration=?1)','FROM deliveries NOT INDEXED WHERE registration=?1)','issuance_work_proof')]
results=[]
def run(label,test):
 with(out/(label+'-compile.txt')).open('w')as f:subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 with(out/(label+'-tests.txt')).open('w')as f:return subprocess.run([str(r/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
for name,before,after,test in cases:
 original=p.read_bytes();s=original.decode();assert s.count(before)==1,(name,s.count(before))
 try:
  p.write_text(s.replace(before,after));broken=p.read_bytes();assert broken!=original
  (out/(name+'-edit.json')).write_text(json.dumps(dict(before=before,after=after,originalSHA256=hashlib.sha256(original).hexdigest(),mutatedSHA256=hashlib.sha256(broken).hexdigest()),indent=2)+'\n')
  assert run(name+'-broken',test)!=0,(name,'escaped')
 finally:p.write_bytes(original);assert p.read_bytes()==original
 assert run(name+'-restored',test)==0
 results.append(dict(rule=name,caught=True,byteRestored=True));(out/'results.json').write_text(json.dumps(results,indent=2)+'\n');print(name,flush=True)
