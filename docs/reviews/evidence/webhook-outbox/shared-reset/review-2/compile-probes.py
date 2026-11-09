from pathlib import Path
import subprocess,json,os
p=Path('/tmp/independent-webhook-round2')
args=json.loads((p/'probe-args.json').read_text())
env=dict(os.environ,RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
r=subprocess.run(args,env=env,text=True,capture_output=True)
(p/'probes-compile.txt').write_text(r.stdout+r.stderr)
raise SystemExit(r.returncode)
