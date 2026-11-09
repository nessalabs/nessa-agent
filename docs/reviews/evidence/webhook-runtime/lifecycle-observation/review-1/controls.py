from pathlib import Path
import os,json,subprocess,hashlib
r=Path(__file__).parent;args=json.loads((r/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
domain=r/'crates/nessa-server/src/webhooks/domain/value_objects/registration_lifecycle.rs';reg=r/'crates/nessa-server/src/webhooks/infrastructure/outbox/registry.rs';restore=r/'crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs'
controls=[('observation-ack',reg,'            self.observe_expiry(credential.clone(), observed_by.clone(), at_ms)\n                .await?;','            let _ = observed_by;','owner_expiry_observation_survives_failed_stop'),('first-cause',domain,'RegistrationPhase::ExpiryObserved => self.finish_observed(),','RegistrationPhase::ExpiryObserved => self.changed(RegistrationPhase::Stopped, StopObservation::new(StopReason::Revoked { initiator }, at_ms)),','all_original_readers_survive_ack_loss'),('diagnostic-phase-correlation',restore,'                audit::Cause::CredentialExpiryObserved => {','                audit::Cause::CredentialExpiryObserved => {\n                    if !matches!(live.as_ref().map(|r| r.lifecycle.phase()), Some(crate::webhooks::domain::RegistrationPhase::ExpiryObserved)) { return Err(OutboxError::Invalid); }','demonstrate_stopped_state_with_unacknowledged_observation')]
results=[]
for name,p,old,new,test in controls:
 original=p.read_bytes();s=original.decode();assert s.count(old)==1
 try:
  p.write_text(s.replace(old,new));assert p.read_bytes()!=original
  (r/(name+'-edit.json')).write_text(json.dumps({'file':str(p.relative_to(r)),'old':old,'new':new,'originalSha256':hashlib.sha256(original).hexdigest()},indent=2)+'\n')
  with (r/(name+'-compile.txt')).open('w') as f:subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
  with (r/(name+'-mutated.txt')).open('w') as f:rc=subprocess.run([str(r/'tests-bin'),test,'--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT).returncode
  assert rc!=0,(name,'escaped')
  if name=='diagnostic-phase-correlation':
   with (r/(name+'-positive.txt')).open('w') as f:subprocess.run([str(r/'tests-bin'),'all_original_readers_survive_ack_loss','--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
 finally:
  p.write_bytes(original);assert p.read_bytes()==original
 with (r/(name+'-restored-compile.txt')).open('w') as f:subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
 with (r/(name+'-restored.txt')).open('w') as f:subprocess.run([str(r/'tests-bin'),test,'--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT,check=True)
 results.append({'name':name,'caught':True,'byteRestored':True,'diagnosticOnly':name.startswith('diagnostic')});(r/'controls.json').write_text(json.dumps(results,indent=2)+'\n');print(name,flush=True)
