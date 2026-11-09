from pathlib import Path
import json,subprocess,os,shutil,hashlib
h=Path('/tmp/nessa-origin-recovery-independent-review');root=Path('/workspace/nessa-webhook-composition')
shutil.copytree(root/'crates/nessa-auth',h/'crates/nessa-auth',dirs_exist_ok=True)
a=json.loads((h/'auth-args.json').read_text());a=[v.replace(str(root/'crates/nessa-auth'),str(h/'crates/nessa-auth')) for v in a]
s=json.loads((h/'server-args.json').read_text());env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
(h/'controlled-auth-args.json').write_text(json.dumps(a,indent=2))
def execute(label):
 for name,args in [('auth',a),('server',s)]:
  with (h/f'{label}-{name}-compile.txt').open('w') as out:r=subprocess.run(args,env=env,stdout=out,stderr=subprocess.STDOUT)
  assert r.returncode==0,(label,name)
 with (h/f'{label}-tests.txt').open('w') as out:r=subprocess.run([str(h/'tests-bin'),'registration_tests'],stdout=out,stderr=subprocess.STDOUT)
 return r.returncode
assert execute('private-baseline')==0
controls=[
 ('receipt-origin','crates/nessa-auth/src/adapters/local/registry.rs','if origin.requires_webhook_recovery() {','if false {'),
 ('product-boundary','crates/nessa-auth/src/adapters/local/registry.rs','if request.purpose == CredentialIssuePurpose::Webhook {\n                let credential = domain_credential(&metadata)?;','if true {\n                let credential = domain_credential(&metadata)?;'),
 ('held-actor','crates/nessa-server/src/webhooks/application/registration.rs','origin.transition() != &expected','false'),
 ('session-cedar','crates/nessa-auth/src/adapters/cedar/policies.cedar','["bootstrap", "admin_issue", "device_pairing", "surface_provision", "owner_recovery"].contains(principal.issuance)','["bootstrap", "admin_issue", "webhook_issue", "device_pairing", "surface_provision", "owner_recovery"].contains(principal.issuance)')
]
results=json.loads((h/'controls.json').read_text())
for label,rel,old,new in controls[1:]:
 p=h/rel;original=p.read_bytes();text=original.decode();assert text.count(old)==1,(label,text.count(old))
 try:
  mutated=text.replace(old,new,1);assert mutated!=text;p.write_text(mutated)
  (h/f'{label}-edit.json').write_text(json.dumps(dict(old=old,new=new,changed_sha256=hashlib.sha256(p.read_bytes()).hexdigest())))
  caught=execute(label+'-mutated')!=0
 finally:p.write_bytes(original)
 restored=execute(label+'-restored')==0
 results.append(dict(rule=label,caught=caught,restored=restored,byte_identical=p.read_bytes()==(root/rel).read_bytes()))
 (h/'controls.json').write_text(json.dumps(results,indent=2)+'\n');assert caught and restored
print(json.dumps(results))
