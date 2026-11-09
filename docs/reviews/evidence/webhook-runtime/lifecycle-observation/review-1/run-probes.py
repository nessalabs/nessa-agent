import json,os,subprocess
from pathlib import Path
r=Path(__file__).parent;env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup');a=json.loads((r/'args.json').read_text())
with (r/'probes-compile.txt').open('w') as f:subprocess.run(a,env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
with (r/'probes.txt').open('w') as f:subprocess.run([str(r/'tests-bin'),'independent_probes','--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
print((r/'probes.txt').read_text())
