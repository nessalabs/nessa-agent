from pathlib import Path
import json,subprocess,os,hashlib
p=Path('/tmp/nessa-assembled-final-review');env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup');a=json.loads((p/'auth-resumed-args.json').read_text());i=a.index('--crate-type');del a[i:i+2];a.insert(1,'--test');a[a.index('-o')+1]=str(p/'auth-probe-tests');b=json.loads((p/'args-resumed.json').read_text())
controls=[('session-origin',p/'crates/nessa-auth/src/adapters/cedar/policies.cedar','["bootstrap", "admin_issue", "device_pairing", "surface_provision", "owner_recovery"]','["bootstrap", "admin_issue", "webhook_issue", "device_pairing", "surface_provision", "owner_recovery"]',a,[str(p/'auth-probe-tests'),'independent_entry_origin','--nocapture']),('cleanup-cutoff',p/'crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs','WITH cutoffs AS MATERIALIZED (','WITH cutoffs AS (',b,[str(p/'tests-bin'),'terminal_cleanup_walks_one_indexed_cutoff_per_registration','--nocapture'])]
results=[]
def run(command,log,ok):
 with(p/log).open('w')as f:r=subprocess.run(command,stdout=f,stderr=subprocess.STDOUT,env=env)
 if ok:assert r.returncode==0,(log,r.returncode)
 else:
  assert r.returncode!=0,(log,'mutation escaped')
  assert '1 failed' in (p/log).read_text(),(log,'test selector did not fail exactly one')
 return r.returncode
for name,file,old,new,compilecmd,test in controls:
 saved=file.read_bytes();s=saved.decode();assert s.count(old)==1
 try:
  file.write_text(s.replace(old,new));assert file.read_bytes()!=saved
  (p/(name+'-edit.json')).write_text(json.dumps({'path':str(file),'before':old,'after':new},indent=2)+'\n')
  run(compilecmd,name+'-mutated-compile.txt',True);run(test,name+'-mutated.txt',False)
 finally:file.write_bytes(saved);assert file.read_bytes()==saved
 run(compilecmd,name+'-restored-compile.txt',True);run(test,name+'-restored.txt',True)
 results.append({'name':name,'caught':True,'restored':hashlib.sha256(file.read_bytes()).hexdigest()});(p/'mutations.json').write_text(json.dumps(results,indent=2)+'\n');print(name,flush=True)
run([str(p/'tests-bin'),'--nocapture'],'final-108.txt',True)
run([str(p/'auth-probe-tests')],'final-auth-215.txt',True)
print('final suites passed',flush=True)
