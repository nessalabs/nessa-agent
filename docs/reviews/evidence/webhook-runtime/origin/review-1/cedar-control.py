from pathlib import Path
import json,subprocess,os,shutil,hashlib
p=Path('/tmp/nessa-origin-independent-review');root=Path('/workspace/nessa-webhook-composition');shutil.copytree(root/'crates/nessa-auth',p/'crates/nessa-auth',dirs_exist_ok=True);a=json.loads((p/'auth-args.json').read_text());a=[x.replace(str(root/'crates/nessa-auth'),str(p/'crates/nessa-auth')) for x in a];server=json.loads((p/'server-args.json').read_text());env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
def call(a,n):
 with (p/n).open('w')as out:r=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT)
 print(n,r.returncode,flush=True);return r.returncode
f=p/'crates/nessa-auth/src/adapters/cedar/policies.cedar';original=f.read_bytes();s=original.decode();guard='"surface_provision", "owner_recovery"].contains';assert s.count(guard)==1
try:
 f.write_text(s.replace(guard,'"surface_provision", "owner_recovery", "webhook_issue"].contains'));assert f.read_bytes()!=original;(p/'cedar-mutated-policy.cedar').write_bytes(f.read_bytes())
 assert call(a,'cedar-mutated-auth-compile.txt')==0
 assert call(server,'cedar-mutated-server-compile.txt')==0
 assert call([str(p/'tests-bin'),'actual_webhook_origin_refuses_bearer'],'cedar-mutated-test.txt')!=0
finally:f.write_bytes(original)
assert call(a,'cedar-restored-auth-compile.txt')==0
assert call(server,'cedar-restored-server-compile.txt')==0
assert call([str(p/'tests-bin'),'registration_tests'],'cedar-restored-tests.txt')==0
(p/'control-restoration.json').write_text(json.dumps({'registration_source_identical':(p/'crates/nessa-server/src/webhooks/application/registration.rs').read_bytes()==(root/'crates/nessa-server/src/webhooks/application/registration.rs').read_bytes(),'cedar_policy_identical':f.read_bytes()==(root/'crates/nessa-auth/src/adapters/cedar/policies.cedar').read_bytes()},indent=2))
