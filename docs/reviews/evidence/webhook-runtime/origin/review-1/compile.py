from pathlib import Path
import json,subprocess,os,shutil
p=Path('/tmp/nessa-origin-independent-review'); root=Path('/workspace/nessa-webhook-composition'); deps=Path('/workspace/nessa-agent/target/debug/deps'); fp=deps.parent/'.fingerprint'
env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
meta=json.loads((fp/'nessa-auth-7a9c812bc923e055/lib-nessa_auth.json').read_text())
a=['/tmp/nessa-cargo/bin/rustc','--edition=2021','--crate-name','nessa_auth','--crate-type','rlib',str(root/'crates/nessa-auth/src/lib.rs'),'-L',f'dependency={deps}','-C','debuginfo=0','-o',str(p/'libnessa_auth.rlib')]
for _,name,_,ident in meta['deps']:
 matches=[]
 for f in fp.glob('*/lib-'+name):
  if int.from_bytes(bytes.fromhex(f.read_text().strip()),'little')==ident:
   suffix=f.parent.name.rsplit('-',1)[1]; artifact=deps/f'lib{name}-{suffix}.rlib'
   if artifact.exists():matches.append(artifact)
 assert len(matches)==1,(name,matches)
 a+=['--extern',f'{name}={matches[0]}']
(p/'auth-args.json').write_text(json.dumps(a,indent=2))
with (p/'auth-compile.txt').open('w') as out:r=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT)
assert r.returncode==0,(p/'auth-compile.txt').read_text()
for rel in ['crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:shutil.copytree(root/rel,p/rel,dirs_exist_ok=True)
driver=Path('/tmp/nessa-origin-probes/lib.rs').read_text().replace('/tmp/nessa-origin-probes',str(p))
(p/'lib.rs').write_text(driver)
a=json.loads((root/'docs/reviews/evidence/webhook-runtime/origin/controls/args.json').read_text())
a=[v.replace('/tmp/nessa-origin-probes',str(p)) for v in a]
a=[v if not v.startswith('nessa_auth=') else f'nessa_auth={p}/libnessa_auth.rlib' for v in a]
(p/'server-args.json').write_text(json.dumps(a,indent=2))
with (p/'server-compile.txt').open('w') as out:r=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT)
assert r.returncode==0,(p/'server-compile.txt').read_text()
with (p/'registration-baseline.txt').open('w') as out:r=subprocess.run([str(p/'tests-bin'),'registration_tests'],stdout=out,stderr=subprocess.STDOUT)
assert r.returncode==0
print('fresh actual-source auth and copied server compilation; registration baseline passed')
