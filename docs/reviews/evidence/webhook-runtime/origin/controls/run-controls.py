from pathlib import Path
import json, subprocess, os, shutil, hashlib
h=Path('/tmp/nessa-origin-probes');root=Path('/workspace/nessa-webhook-composition')
for rel in ['crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:shutil.copytree(root/rel,h/rel,dirs_exist_ok=True)
env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
args=json.loads((h/'args.json').read_text())
def compile_to(name):
 with (h/(name+'-compile.txt')).open('w') as out: r=subprocess.run(args,env=env,stdout=out,stderr=subprocess.STDOUT)
 if r.returncode:raise RuntimeError(name+' did not compile')
def run(name,test):
 with (h/(name+'.txt')).open('w') as out:return subprocess.run([str(h/'tests-bin'),test],stdout=out,stderr=subprocess.STDOUT).returncode
compile_to('controls-baseline')
if run('controls-baseline','registration_tests')!=0:raise RuntimeError('baseline failed')
p=h/'crates/nessa-server/src/webhooks/application/registration.rs';original=p.read_bytes();s=original.decode()
controls=[
 ('held original actor','origin.transition() != &expected','actual_original_actor'),
 ('held original command','origin.correlation() != Some(intent.issue.request_id.as_str())','actual_original_command'),
 ('committed origin revision','!origin.matches(&credential, hold.snapshot.revision)','matching_reply_cannot_turn_a_future_origin'),
 ('journal sequence','transitions[0].sequence != origin.sequence()','contradictory_auth_reply'),
 ('journal revision','transitions[0].revision != origin.revision()','contradictory_auth_reply'),
 ('typed ambiguous recovery','origin.cause() == IssuanceCause::AdminIssue','ordinary_origin_requires_explicit_recovery'),
]
results=[]
try:
 for i,(rule,expression,test) in enumerate(controls):
  assert s.count(expression)==1,(rule,s.count(expression))
  p.write_text(s.replace(expression,'false',1));compile_to(f'{i}-mutated');caught=run(f'{i}-mutated',test)!=0
  p.write_bytes(original);compile_to(f'{i}-restored');restored=run(f'{i}-restored',test)==0
  row=dict(index=i,rule=rule,test=test,caught=caught,restored=restored);results.append(row)
  (h/'results.json').write_text(json.dumps(results,indent=2)+'\n')
  with (h/'progress.txt').open('a') as out:out.write(f'{i}: caught={caught} restored={restored}\n')
  if not caught or not restored:raise RuntimeError(str(row))
finally:p.write_bytes(original)
assert p.read_bytes()==(root/'crates/nessa-server/src/webhooks/application/registration.rs').read_bytes()
(h/'source-match.json').write_text(json.dumps(dict(source='a3f5578e1f626cefe031fa04d6d61c6df4571a10',sha256=hashlib.sha256(original).hexdigest(),restored_identical=True),indent=2)+'\n')
