import json,os,shutil,subprocess
from pathlib import Path
src=Path('/workspace/nessa-webhook-composition');out=Path('/tmp/nessa-retained-progress-independent-review');template=Path('/tmp/nessa-expiry-context-final-review')
for rel in ['crates/nessa-auth','crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:
 shutil.copytree(src/rel,out/rel,dirs_exist_ok=True)
(out/'lib.rs').write_text((template/'lib.rs').read_text().replace(str(template),str(out)))
a=json.loads((template/'auth-args.json').read_text());a=[v.replace('/workspace/nessa-webhook-composition',str(out)).replace(str(template),str(out)) for v in a];(out/'auth-args.json').write_text(json.dumps(a,indent=2)+'\n')
b=json.loads((template/'args.json').read_text());b=[v.replace(str(template),str(out)) for v in b];(out/'args.json').write_text(json.dumps(b,indent=2)+'\n')
env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
for label,args in [('auth',a),('owner',b),('baseline',[str(out/'tests-bin'),'--nocapture'])]:
 with(out/(label+'.txt')).open('w')as f:subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
print('Private baseline complete',flush=True)
