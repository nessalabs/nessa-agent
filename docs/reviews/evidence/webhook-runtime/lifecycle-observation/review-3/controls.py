import hashlib,json,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-w24-m2-independent-review');src=p/'crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs';args=json.loads((p/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
def run(label,test):
 with(p/(label+'-compile.txt')).open('w')as f:subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 with(p/(label+'-tests.txt')).open('w')as f:return subprocess.run([str(p/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
s=src.read_bytes();text=s.decode();cases=[('owed-issue','audit::Cause::CredentialIssue => {\n                    if owed','audit::Cause::CredentialIssue => {\n                    if false && owed','independent_owed_issue_expiry'),('published-exemption','audit::Cause::CredentialIssue => {\n                    if owed','audit::Cause::CredentialIssue => {\n                    if true','published_original_issue_remains'),('work-proof','SELECT EXISTS(SELECT 1 FROM cursors WHERE registration=?1) OR EXISTS(SELECT 1 FROM deliveries WHERE registration=?1)','SELECT 0','completed_issue_cannot_be_owed')]
rows=[]
for name,before,after,test in cases:
 assert text.count(before)==1,(name,text.count(before))
 try:
  changed=text.replace(before,after).encode();src.write_bytes(changed);assert src.read_bytes()!=s
  (p/(name+'-edit.json')).write_text(json.dumps(dict(before=before,after=after,originalSHA256=hashlib.sha256(s).hexdigest(),changedSHA256=hashlib.sha256(changed).hexdigest()),indent=2)+'\n')
  assert run(name+'-broken',test)!=0
 finally:src.write_bytes(s);assert src.read_bytes()==s
 assert run(name+'-restored',test)==0
 rows.append(dict(rule=name,caught=True,byteRestored=True));(p/'controls.json').write_text(json.dumps(rows,indent=2)+'\n')
 print(name,flush=True)
# Diagnostic structural alternative only, never a checkout fix or qualification.
before='EXISTS(SELECT 1 FROM deliveries WHERE registration=?1)";';after='EXISTS(SELECT 1 FROM deliveries WHERE registration=?1) OR EXISTS(SELECT 1 FROM source_reset_facts WHERE registration=?1)";';assert text.count(before)==1
try:
 src.write_text(text.replace(before,after));assert src.read_bytes()!=s
 (p/'diagnostic-source-proof-edit.json').write_text(json.dumps(dict(before=before,after=after,originalSHA256=hashlib.sha256(s).hexdigest(),changedSHA256=hashlib.sha256(src.read_bytes()).hexdigest()),indent=2)+'\n')
 assert run('diagnostic-source-proof','required_initial_rejection')==0
finally:src.write_bytes(s);assert src.read_bytes()==s
assert run('required-restored-failure','required_initial_rejection')!=0
print('Controls and diagnostic complete',flush=True)
