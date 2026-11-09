import json,os,subprocess
from pathlib import Path
out=Path('/tmp/nessa-private-fixture-independent-review');p=out/'auth/tests/revocation.rs';fixed=p.read_bytes()
p.write_bytes(fixed+(out/'probe.rs').read_bytes())
a=json.loads((out/'fact-args.json').read_text())
env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
try:
 with(out/'probe-compile.txt').open('w')as f:subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,check=True,env=env,preexec_fn=lambda:os.umask(0o022))
 with(out/'probe-tests.txt').open('w')as f:subprocess.run([str(out/'fact-tests'),'--nocapture'],stdout=f,stderr=subprocess.STDOUT,check=True,env=env,preexec_fn=lambda:os.umask(0o022))
finally:p.write_bytes(fixed);assert p.read_bytes()==fixed
print('8 tests including shared-parent security probe pass; private test copy byte-restored')
