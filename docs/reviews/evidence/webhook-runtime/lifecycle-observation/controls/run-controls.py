import json,os,subprocess
from pathlib import Path
r=Path('/tmp/nessa-lifecycle-frozen'); out=Path('/tmp/nessa-w24-controls'); out.mkdir(exist_ok=True)
args=json.loads((r/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
domain=r/'crates/nessa-server/src/webhooks/domain/value_objects/registration_lifecycle.rs'
registry=r/'crates/nessa-server/src/webhooks/infrastructure/outbox/registry.rs'
restore=r/'crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs'
saved={p:p.read_bytes() for p in [domain,registry,restore]}
controls=[('observation-required',domain,'RegistrationPhase::Active => Err(LifecycleError::ObservationRequired),','RegistrationPhase::Active => Ok((self.clone(), None)),','expiry_requires_observation'),
('ack-before-stop',registry,'            self.observe_expiry(credential.clone(), observed_by.clone(), at_ms)\n                .await?;','            let _ = observed_by;','owner_expiry_observation_survives'),
('first-cause',domain,'RegistrationPhase::ExpiryObserved => self.finish_observed(),','RegistrationPhase::ExpiryObserved => self.changed(RegistrationPhase::Stopped, StopObservation::new(StopReason::Revoked { initiator }, at_ms)),','actual_revocation_stops_directly'),
('observed-context',restore,'if observed\n                        != Some(&audit::check_stop(','if false && observed\n                        != Some(&audit::check_stop(','owed_observation_and_retired_stop')]
results=[]
for name,path,before,after,test in controls:
 original=path.read_text();assert original.count(before)==1,(name,original.count(before))
 try:
  path.write_text(original.replace(before,after));assert path.read_bytes()!=saved[path]
  (out/(name+'-edit.txt')).write_text(before+'\n=>\n'+after+'\n')
  with (out/(name+'-compile.txt')).open('w') as f: subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
  with (out/(name+'-failed.txt')).open('w') as f: rc=subprocess.run([str(r/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
  assert rc!=0,(name,'mutation escaped')
 finally:
  path.write_bytes(saved[path]);assert path.read_bytes()==saved[path]
 with (out/(name+'-restored-compile.txt')).open('w') as f: subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 with (out/(name+'-restored.txt')).open('w') as f: subprocess.run([str(r/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 results.append({'control':name,'caught':True,'byte_restored':True});(out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
 print(name,flush=True)
