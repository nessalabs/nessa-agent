from pathlib import Path
import os,subprocess,json,shutil
root=Path('/workspace/nessa-webhook-runtime');probe=Path('/tmp/nessa-registration-repair')
for part in ['src/webhooks','tests/webhooks']:
 shutil.copytree(root/'crates/nessa-server'/part,probe/'crates/nessa-server'/part,dirs_exist_ok=True)
base=probe/'crates/nessa-server/src/webhooks'
args=json.loads((probe/'args.json').read_text());args[args.index('-o')+1]=str(probe/'mutated-bin')
env=os.environ|dict(RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
original={p:p.read_bytes() for p in base.rglob('*') if p.is_file()};rules=[]
def rule(file,old,new,test):rules.append((file,old,new,test))
app='application/registration.rs';store='infrastructure/outbox/intents.rs'
first='registration_commits_real_auth'
rule(app,'.resolve(&proposed)','.resolve(&reserved)','substituted_reservation_is_rejected')
rule(store,'return saved.resolve(&proposed);','return Ok(saved);','conflicting_config_and_caller_inputs')
for selection in ['selected.id = self.id;','selected.issue.credential_id = self.issue.credential_id.clone();','selected.issue.issued_at = self.issue.issued_at;','selected.observed_at_ms = self.observed_at_ms;']:
 rule(app,selection,'',first)
rule(app,'self.issue == other.issue','true','substituted_reservation_is_rejected')
rule(app,'self.config.destination == other.config.destination','true','substituted_reservation_is_rejected')
rule(app,'self.observed_at_ms == other.observed_at_ms','true','installation_requires_the_retained_original_intent')
rule(app,'&& bool::from(\n                self.config\n                    .key\n                    .expose_bytes()\n                    .ct_eq(other.config.key.expose_bytes()),\n            )','','substituted_reservation_is_rejected')
rule(store,'let intent = row.map_err(super::cursor::row_error)?.checked()?;','let intent = row.map_err(invalid)?.checked()?;','transient_intent_scan_interruption')
rule(app,'self.registration()?;','','substituted_reservation_is_rejected')
results=[]
def compile():
 r=subprocess.run(args,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 assert r.returncode==0,r.stdout.decode()[-3000:]
try:
 compile()
 baseline=subprocess.run([str(probe/'mutated-bin'),'registration','--test-threads=4'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 (probe/'baseline.txt').write_bytes(baseline.stdout);assert baseline.returncode==0,baseline.stdout.decode()
 for i,(file,old,new,test) in enumerate(rules):
  path=base/file;src=original[path].decode();assert src.count(old)==1,(i,file,old,src.count(old))
  path.write_text(src.replace(old,new));compile()
  r=subprocess.run([str(probe/'mutated-bin'),test],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
  (probe/f'{i}-mutated.txt').write_bytes(r.stdout)
  path.write_bytes(original[path]);compile()
  c=subprocess.run([str(probe/'mutated-bin'),test],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
  (probe/f'{i}-restored.txt').write_bytes(c.stdout)
  results.append(dict(index=i,file=file,rule=old,test=test,caught=r.returncode!=0,restored=c.returncode==0))
  (probe/'results.json').write_text(json.dumps(results,indent=2)+'\n')
  print(i,'caught' if r.returncode else 'NOT CAUGHT','restored' if c.returncode==0 else 'BAD CONTROL',flush=True)
finally:
 for path,data in original.items():path.write_bytes(data)
 print('all copied source restored:',all(p.read_bytes()==b for p,b in original.items()),flush=True)
