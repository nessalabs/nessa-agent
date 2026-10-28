import json,os,subprocess,hashlib
from pathlib import Path
p=Path('/tmp/nessa-revocation-independent-review');a=json.loads((p/'auth-args.json').read_text());b=json.loads((p/'fact-args.json').read_text());env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
def command(args,name,expected):
 with (p/name).open('w') as f:r=subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT)
 assert (r.returncode==0)==expected,(name,r.returncode)
cases=[('required-fact','application/ports.rs','(Some(_), None) => Err(AccessError::Unavailable),','(Some(_), None) => Ok(None),'),('actual-actor','adapters/local/registry.rs','''                            record.clone(),
                            &credential,''','''                            { let mut fabricated = record.clone(); fabricated.initiator = InitiatorDto::LocalOperator; fabricated },
                            &credential,'''),('time-binding','application/revocation.rs','&& self.transition.after().revoked_at == current.revoked_at()','')]
results=[]
for label,rel,old,new in cases:
 f=p/'auth/src'/rel;original=f.read_text();assert original.count(old)==1,(label,original.count(old));changed=original.replace(old,new);assert changed!=original
 original_hash=hashlib.sha256(original.encode()).hexdigest();changed_hash=hashlib.sha256(changed.encode()).hexdigest();(p/f'{label}-edit.json').write_text(json.dumps(dict(path=rel,old=old,new=new,originalHash=original_hash,changedHash=changed_hash),indent=2)+'\n')
 try:
  f.write_text(changed);assert hashlib.sha256(f.read_bytes()).hexdigest()==changed_hash
  command(a,f'{label}-broken-auth-compile.txt',True);command(b,f'{label}-broken-test-compile.txt',True);command([str(p/'fact-tests'),'--nocapture'],f'{label}-broken-tests.txt',False)
 finally:f.write_text(original)
 assert hashlib.sha256(f.read_bytes()).hexdigest()==original_hash
 command(a,f'{label}-restored-auth-compile.txt',True);command(b,f'{label}-restored-test-compile.txt',True);command([str(p/'fact-tests'),'--nocapture'],f'{label}-restored-tests.txt',True)
 results.append(dict(rule=label,caught=True,byteRestored=True));print(label,'caught/restored',flush=True)
(p/'controls.json').write_text(json.dumps(results,indent=2)+'\n')
c=b.copy();c[c.index('--test')+1]=str(p/'auth/tests/origin.rs');c[c.index('-o')+1]=str(p/'origin-tests');command(c,'origin-compile.txt',True);command([str(p/'origin-tests')],'origin-tests.txt',True)
rows=[];src=Path('/workspace/nessa-webhook-revocation/crates/nessa-auth/src')
for f in src.rglob('*.rs'):
 q=p/'auth/src'/f.relative_to(src);assert q.read_bytes()==f.read_bytes();rows.append(dict(path=str(f.relative_to(src)),sha256=hashlib.sha256(f.read_bytes()).hexdigest()))
(p/'production-source-provenance.json').write_text(json.dumps(dict(head='5d2d57d0223c2a2cc0bd040ec54d02666cc24380',files=rows),indent=2)+'\n')
print('Three controls and original-origin5tests passed',flush=True)
