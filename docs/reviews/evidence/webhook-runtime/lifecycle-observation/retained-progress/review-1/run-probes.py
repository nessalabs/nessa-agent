import json,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-retained-progress-independent-review');a=json.loads((p/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'probes-compile.txt').open('w')as o:subprocess.run(a,stdout=o,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'probes-tests.txt').open('w')as o:subprocess.run([str(p/'tests-bin'),'independent_','--nocapture'],stdout=o,stderr=subprocess.STDOUT,check=True,env=env)
