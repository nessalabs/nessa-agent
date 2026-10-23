from pathlib import Path
import json,shutil,subprocess,os,hashlib
root=Path('/workspace/nessa-webhook-composition'); h=Path('/tmp/nessa-origin-recovery-controls');h.mkdir(exist_ok=True)
for rel in ['crates/nessa-auth','crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:
 shutil.copytree(root/rel,h/rel,dirs_exist_ok=True)
(h/'lib.rs').write_text(Path('/tmp/nessa-origin-probes/lib.rs').read_text().replace('/tmp/nessa-origin-probes',str(h)))
a=json.loads(Path('/tmp/nessa-origin-independent-review/auth-args.json').read_text());a=[v.replace(str(root),str(h)).replace('/tmp/nessa-origin-independent-review',str(h)) for v in a]
s=json.loads(Path('/tmp/nessa-origin-probes/args.json').read_text());s=[v.replace('/tmp/nessa-origin-probes',str(h)) for v in s];s=[v if not v.startswith('nessa_auth=') else f'nessa_auth={h}/libnessa_auth.rlib' for v in s]
(h/'auth-args.json').write_text(json.dumps(a,indent=2));(h/'server-args.json').write_text(json.dumps(s,indent=2))
env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
def compile_run(label,test):
 for kind,args in [('auth',a),('server',s)]:
  with (h/f'{label}-{kind}-compile.txt').open('w') as out:
   rc=subprocess.run(args,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
  assert rc==0,(label,kind)
 with (h/f'{label}-test.txt').open('w') as out:return subprocess.run([str(h/'tests-bin'),test],stdout=out,stderr=subprocess.STDOUT).returncode
assert compile_run('baseline','registration_tests')==0
controls=[('actual-receipt-origin','crates/nessa-auth/src/adapters/local/registry.rs','if request.purpose == CredentialIssuePurpose::Webhook {','if false {','ambiguous_historical_admin_origin'),('product-semantics','crates/nessa-auth/src/adapters/local/registry.rs','if request.purpose == CredentialIssuePurpose::Webhook {','if true {','registration_tests'),('typed-forwarding','crates/nessa-server/src/webhooks/application/registration.rs','return Err(RegistrationFailure::OriginRecoveryRequired);','return Err(RegistrationFailure::Auth(CredentialAdminError::Unavailable));','ambiguous_historical_admin_origin')]
results=[]
for label,rel,old,new,test in controls:
 p=h/rel;original=p.read_bytes();text=original.decode();assert old in text
 try:
  p.write_text(text.replace(old,new,1));caught=compile_run(label+'-mutated',test)!=0
 finally:p.write_bytes(original)
 restored=compile_run(label+'-restored',test)==0
 results.append(dict(rule=label,caught=caught,restored=restored,identical=p.read_bytes()==(root/rel).read_bytes()))
 (h/'results.json').write_text(json.dumps(results,indent=2)+'\n');assert caught and restored
print(json.dumps(results))
