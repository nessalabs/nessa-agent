from pathlib import Path
import json,subprocess,shutil,os,hashlib
root=Path('/workspace/nessa-webhook-retention');old=Path('/tmp/nessa-retention-oldhead-probes');h=Path('/tmp/nessa-retention-fixed-profile');h.mkdir(exist_ok=True)
for rel in ['crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:shutil.copytree(root/rel,h/rel,dirs_exist_ok=True)
fixture=Path('crates/nessa-server/tests/webhooks/infrastructure/outbox.rs');shutil.copyfile(old/fixture,h/fixture)
(h/'lib.rs').write_text((old/'lib.rs').read_text().replace(str(old),str(h)))
a=json.loads((old/'args.json').read_text());a=[v.replace(str(old),str(h)) for v in a];a=[v if not v.startswith('nessa_auth=') else f'nessa_auth={old}/libnessa_auth.rlib' for v in a]
(h/'args.json').write_text(json.dumps(a,indent=2));env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
(h/'source-match.json').write_text(json.dumps({'fixture_sha256':hashlib.sha256((h/fixture).read_bytes()).hexdigest(),'fixture_identical':(h/fixture).read_bytes()==(old/fixture).read_bytes(),'source_matches':{str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [root/'crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs',root/'crates/nessa-server/src/webhooks/infrastructure/outbox/schema.sql']}},indent=2))
with (h/'compile.txt').open('w') as out:rc=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
assert rc==0,'fixed profile compile'
for label,name in [('terminal','terminal_retention_is_bounded'),('stop','stop_retention_is_bounded_when_publication_fails')]:
 command=['strace','-f','-c','-e','trace=fsync,fdatasync,openat,rename,renameat,link,linkat,unlink,fcntl,pwrite64,write','-o',str(h/(label+'-syscalls.txt')),str(h/'tests-bin'),'--exact','webhooks::infrastructure::outbox::store::tests::'+name,'--nocapture'];(h/(label+'-command.json')).write_text(json.dumps(command,indent=2))
 with (h/(label+'-profile.txt')).open('w') as out:rc=subprocess.run(command,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
 assert rc==0,(label,rc)
 print(label,'passed',flush=True)
