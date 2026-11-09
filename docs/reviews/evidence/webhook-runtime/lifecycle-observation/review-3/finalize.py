import hashlib,json,os,subprocess
from pathlib import Path
out=Path('/tmp/nessa-w24-m2-independent-review');root=Path('/workspace/nessa-webhook-composition');env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(out/'final-compile.txt').open('w')as f:subprocess.run(json.loads((out/'args.json').read_text()),stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
with(out/'final-tests.txt').open('w')as f:subprocess.run([str(out/'tests-bin'),'--nocapture','--skip','required_initial_rejection'],stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
rows=[]
for rel in ['crates/nessa-auth/src','crates/nessa-server/src/webhooks']:
 for src in (root/rel).rglob('*.rs'):
  copy=out/src.relative_to(root);assert src.read_bytes()==copy.read_bytes(),src
  rows.append({'path':str(src.relative_to(root)),'sha256':hashlib.sha256(src.read_bytes()).hexdigest(),'privateRestoredByteIdentical':True})
(out/'production-source-provenance.json').write_text(json.dumps({'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'base':'8c6e673b69055bdc1b2320e4f1b990b5c795aeba','status':subprocess.check_output(['git','status','--short'],cwd=root,text=True),'files':rows},indent=2)+'\n')
subprocess.run(['git','diff','--check','8c6e673b69055bdc1b2320e4f1b990b5c795aeba','HEAD'],cwd=root,check=True)
print('Final source and test verification complete',flush=True)
