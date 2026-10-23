from pathlib import Path
import json,subprocess,os,shutil
p=Path('/tmp/nessa-origin-independent-review');a=json.loads((p/'auth-args.json').read_text());i=a.index('--crate-type');del a[i:i+2];a.insert(1,'--test');a[a.index('-o')+1]=str(p/'auth-unit-tests');env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with (p/'auth-unit-compile.txt').open('w') as out:r=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT)
print('compile',r.returncode,flush=True)
if r.returncode==0:
 with (p/'auth-unit.txt').open('w') as out:r=subprocess.run([str(p/'auth-unit-tests')],env=env,stdout=out,stderr=subprocess.STDOUT)
 print('tests',r.returncode,flush=True)
