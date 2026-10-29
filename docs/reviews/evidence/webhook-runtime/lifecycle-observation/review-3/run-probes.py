import json,os,subprocess
from pathlib import Path
p=Path('/tmp/nessa-w24-m2-independent-review');env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'probes-compile.txt').open('w')as f:subprocess.run(json.loads((p/'args.json').read_text()),stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'probes.txt').open('w')as f:r=subprocess.run([str(p/'tests-bin'),'independent_probes','--nocapture','--skip','required_initial_rejection'],stdout=f,stderr=subprocess.STDOUT,env=env)
print((p/'probes.txt').read_text());raise SystemExit(r.returncode)
