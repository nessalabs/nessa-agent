import hashlib,json,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-retained-progress-independent-review');src=Path('/workspace/nessa-webhook-composition');checks=[]
for rel in ['crates/nessa-auth/src','crates/nessa-server/src/webhooks']:
 for file in (src/rel).rglob('*'):
  if file.is_file():
   copied=p/file.relative_to(src);assert copied.read_bytes()==file.read_bytes(),str(file)
   checks.append(dict(path=str(file.relative_to(src)),sha256=hashlib.sha256(file.read_bytes()).hexdigest()))
(p/'source-provenance.json').write_text(json.dumps(dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=src,text=True).strip(),productionCopiesByteEqual=True,files=checks),indent=2)+'\n')
a=json.loads((p/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'final-compile.txt').open('w')as f:subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'final-tests.txt').open('w')as f:subprocess.run([str(p/'tests-bin'),'--nocapture'],stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
