import json,subprocess,os
from pathlib import Path
p=Path('/tmp/nessa-revocation-independent-review');t=p/'auth/tests/revocation.rs';t.write_text(t.read_text()+'\n'+(p/'probes.rs').read_text());a=json.loads((p/'fact-args.json').read_text());env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
for name,args in [('probes-compile.txt',a),('probes-tests.txt',[str(p/'fact-tests'),'--nocapture'])]:
 with (p/name).open('w') as f:r=subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT)
 assert r.returncode==0,(name,r.returncode)
print('Independent3probes plus7baseline passed')
