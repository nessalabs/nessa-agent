import json,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-w24-m2-independent-review');a=json.loads((p/'auth-args.json').read_text());i=a.index('--crate-type');a[i:i+2]=['--test'];a[a.index('-o')+1]=str(p/'unit-tests');env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'unit-compile.txt').open('w')as f:subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'unit-tests.txt').open('w')as f:subprocess.run([str(p/'unit-tests')],stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
