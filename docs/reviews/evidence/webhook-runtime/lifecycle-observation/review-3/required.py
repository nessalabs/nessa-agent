import json,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-w24-m2-independent-review');a=json.loads((p/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'required-compile.txt').open('w')as f:subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'required-rejection-fails.txt').open('w')as f:rc=subprocess.run([str(p/'tests-bin'),'required_initial_rejection','--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
assert rc!=0
