from pathlib import Path
import json,subprocess,os
p=Path('/tmp/nessa-assembled-independent-review');a=json.loads((p/'auth-resumed-args.json').read_text());i=a.index('--crate-type');del a[i:i+2];a.insert(1,'--test');a[a.index('-o')+1]=str(p/'auth-unit-tests');env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
for command,log in [(a,'auth-unit-compile.txt'),([str(p/'auth-unit-tests')],'auth-unit.txt')]:
 with (p/log).open('w')as f:subprocess.run(command,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
print((p/'auth-unit.txt').read_text()[-250:])
