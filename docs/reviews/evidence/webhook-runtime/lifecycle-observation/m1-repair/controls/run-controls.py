import json,os,subprocess
from pathlib import Path
r=Path('/tmp/nessa-lifecycle-frozen');out=Path('/tmp/nessa-w24-context-controls');out.mkdir(exist_ok=True)
args=json.loads((r/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
base=r/'crates/nessa-server/src/webhooks'
rest=base/'infrastructure/outbox/restoration.rs';domain=base/'domain/value_objects/registration_lifecycle.rs';queue=base/'infrastructure/outbox/queue.rs'
guard='''                    if owed
                        && !matches!(&live, Some(live) if live.lifecycle.phase() == RegistrationPhase::ExpiryObserved)
                    {
                        return Err(OutboxError::Invalid);
                    }
'''
controls=[('owed-phase',rest,guard,'','owed_observation_cannot_restore',1),('work-phase',rest,'    validate_queued(connection)?;\n','','stopped_cannot_retain_queued',2),('queued-owner',domain,'RegistrationPhase::Stopped => false,','RegistrationPhase::Stopped => true,','stopped_cannot_retain_queued',1),('queue-admission',queue,'if !current_registration.is_some_and','if false && !current_registration.is_some_and','stopped_queue_admission',1),('indexed-work',rest,'FROM deliveries INDEXED BY ready_deliveries WHERE','FROM deliveries NOT INDEXED WHERE','queued_lifecycle_selection_does_not_walk',1),('bounded-work',rest,'if index == MAX_REGISTRATIONS {','if false && index == MAX_REGISTRATIONS {','queued_lifecycle_selection_refuses',1)]
results=[]
for name,path,before,after,test,count in controls:
 original=path.read_bytes();text=original.decode();assert text.count(before)==count,(name,text.count(before))
 try:
  path.write_text(text.replace(before,after));assert path.read_bytes()!=original
  (out/(name+'-edit.txt')).write_text(before+'\n=>\n'+after+'\n')
  with(out/(name+'-compile.txt')).open('w')as f:subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
  with(out/(name+'-failed.txt')).open('w')as f:rc=subprocess.run([str(r/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
  assert rc!=0,(name,'escaped')
 finally:path.write_bytes(original);assert path.read_bytes()==original
 with(out/(name+'-restored-compile.txt')).open('w')as f:subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 with(out/(name+'-restored.txt')).open('w')as f:subprocess.run([str(r/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
 results.append({'control':name,'caught':True,'byte_restored':True});(out/'results.json').write_text(json.dumps(results,indent=2)+'\n');print(name,flush=True)
