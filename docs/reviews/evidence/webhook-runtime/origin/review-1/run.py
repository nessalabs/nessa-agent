from pathlib import Path
import os,json,subprocess,shutil
p=Path('/tmp/nessa-origin-independent-review');env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup');args=json.loads((p/'server-args.json').read_text())
def call(a,n):
 with (p/n).open('w') as out:r=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT)
 print(n,r.returncode,flush=True);return r.returncode
assert call(args,'adjacent-compile.txt')==0
assert call([str(p/'tests-bin'),'registration_tests'],'registration-expanded.txt')==0
# Compile the current origin integration tests against the independently built auth.
a=[args[0],'--edition=2021','--test','/workspace/nessa-webhook-composition/crates/nessa-auth/tests/origin.rs','-L','dependency=/workspace/nessa-agent/target/debug/deps','-o',str(p/'origin-tests')]
for i,v in enumerate(args):
 if v=='--extern' and args[i+1].split('=')[0] in ['nessa_auth','serde_json']:a+=['--extern',args[i+1]]
assert call(a,'origin-compile.txt')==0
assert call([str(p/'origin-tests')],'origin-tests.txt')==0
# Registration actor comparison mutation, isolated and restored.
f=p/'crates/nessa-server/src/webhooks/application/registration.rs';original=f.read_bytes();s=original.decode();guard='origin.transition() != &expected';assert s.count(guard)==1
try:
 f.write_text(s.replace(guard,'false'));assert f.read_bytes()!=original
 (p/'actor-mutated-hunk.txt').write_text(s[s.index('if !origin.matches'):s.index('let registration_id')].replace(guard,'false'))
 assert call(args,'actor-mutated-compile.txt')==0
 assert call([str(p/'tests-bin'),'actual_original_actor'],'actor-mutated-test.txt')!=0
finally:f.write_bytes(original)
assert call(args,'actor-restored-compile.txt')==0
assert call([str(p/'tests-bin'),'registration_tests'],'actor-restored-tests.txt')==0
print('complete',flush=True)
