import json,os,shutil,subprocess
from pathlib import Path
src=Path('/workspace/nessa-webhook-revocation'); out=Path('/tmp/nessa-revocation-controls'); out.mkdir(exist_ok=True)
shutil.copytree(src/'crates/nessa-auth',out/'auth',dirs_exist_ok=True)
env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
a=json.loads(Path('/tmp/nessa-retention-independent-review/auth-resumed-args.json').read_text()); a=[v.replace('/workspace/nessa-webhook-retention/crates/nessa-auth',str(out/'auth')).replace('/tmp/nessa-retention-independent-review',str(out)) for v in a]
b=['/tmp/nessa-cargo/bin/rustc','--edition=2021','--test',str(out/'auth/tests/revocation.rs'),'-L','dependency=/tmp/nessa-retention-dependency-snapshot','--extern',f'nessa_auth={out}/libnessa_auth.rlib','--extern','serde_json=/tmp/nessa-retention-dependency-snapshot/libserde_json-004624a94ad74dab.rlib','--extern','tempfile=/tmp/nessa-retention-dependency-snapshot/libtempfile-91dda1363cb9b4ae.rlib','-o',str(out/'tests')]
def run(label):
 for name,args in [('auth',a),('test-compile',b)]:
  with (out/f'{label}-{name}.txt').open('w') as f:
   p=subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,env=env)
  if p.returncode: raise RuntimeError(f'{label} {name} failed')
 with (out/f'{label}-tests.txt').open('w') as f: p=subprocess.run([str(out/'tests'),'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env)
 return p.returncode
assert run('baseline')==0
cases=[
 ('revoked-cause','src/application/revocation.rs','!matches!(transition.cause(), TransitionCause::Revoked(_))','false'),
 ('missing-fact','src/application/ports.rs','(Some(_), None) => Err(AccessError::Unavailable),','(Some(_), None) => Ok(None),'),
 ('sequence-order','src/application/ports.rs','&& fact.sequence() > self.origin.sequence()',''),
 ('revision-order','src/application/ports.rs','&& fact.revision() >= self.origin.revision()',''),
 ('snapshot-revision','src/application/revocation.rs','self.journal.revision() <= revision','(revision == revision)' ),
 ('credential-identity','src/application/revocation.rs','&& self.transition.credential_id() == current.id()',''),
 ('issuance-time','src/application/revocation.rs','&& self.transition.after().issued_at == current.issued_at()',''),
 ('expiry-time','src/application/revocation.rs','&& self.transition.after().expires_at == current.expires_at()',''),
 ('revocation-time','src/application/revocation.rs','&& self.transition.after().revoked_at == current.revoked_at()',''),
 ('actual-reader','src/adapters/local/registry.rs','let revocation = if credential.revoked_at().is_some() {','let revocation = if false {'),
 ('journal-sequence','src/application/credential_journal.rs','record.sequence == 0','false'),
 ('journal-revision','src/application/credential_journal.rs','record.revision == 0','false'),
 ('journal-correlation','src/application/credential_journal.rs','id.trim().is_empty() || id.len() > 200','{ let _ = id; false }'),
]
results=[]
for label,rel,old,new in cases:
 path=out/'auth'/rel; original=path.read_text(); assert original.count(old)==1,(label,original.count(old))
 path.write_text(original.replace(old,new))
 try: code=run(label)
 finally: path.write_text(original)
 results.append(dict(rule=label,mutation_detected=code!=0))
 (out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
 print(label,code,flush=True)
assert all(r['mutation_detected'] for r in results)
assert run('restored')==0
print('All controls detected; restored baseline passed',flush=True)
