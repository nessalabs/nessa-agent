from pathlib import Path
import subprocess,json,os
p=Path('/tmp/independent-shared-reset-review')
args=json.loads((p/'args.json').read_text())
args[args.index('-o')+1]=str(p/'probes-bin')
env=dict(os.environ,RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
r=subprocess.run(args,env=env,text=True,capture_output=True)
(p/'probes-compile.txt').write_text(r.stdout+r.stderr)
print('compile',r.returncode)
if r.returncode:raise SystemExit(r.returncode)
r=subprocess.run([str(p/'probes-bin'),'independent_'],env=env,text=True,capture_output=True,timeout=120)
(p/'probes.txt').write_text(r.stdout+r.stderr)
print(r.stdout)
raise SystemExit(r.returncode)
