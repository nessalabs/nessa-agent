import pathlib,json,subprocess,os,hashlib
r=pathlib.Path('/tmp/nessa-expiry-context-final-review'); source=r/'crates/nessa-server/src/webhooks'; args=json.loads((r/'args.json').read_text()); env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup'); results=[]
cases=[('owed-phase',source/'infrastructure/outbox/restoration.rs', '''                    if owed
                        && !matches!(&live, Some(live) if live.lifecycle.phase() == RegistrationPhase::ExpiryObserved)
                    {
                        return Err(OutboxError::Invalid);
                    }
''','',1,'independent_m1_three_forms'),
('work-phase',source/'infrastructure/outbox/restoration.rs','    validate_queued(connection)?;\n','',2,'independent_m1_three_forms'),
('domain-queued',source/'domain/value_objects/registration_lifecycle.rs','            RegistrationPhase::Stopped => false,','            RegistrationPhase::Stopped => true,',1,'stopped_queue_admission_refuses')]
def run(name,selector):
 with (r/(name+'-compile.txt')).open('w') as f:p=subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT)
 assert p.returncode==0,(name,'compile')
 with (r/(name+'.txt')).open('w') as f:p=subprocess.run([str(r/'tests-bin'),selector,'--test-threads=1','--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT)
 return p.returncode
for name,path,old,new,count,selector in cases:
 original=path.read_bytes();text=original.decode();assert text.count(old)==count,(name,text.count(old));edited=text.replace(old,new);assert edited!=text
 try:
  path.write_text(edited);assert path.read_bytes()!=original
  proof={'name':name,'path':str(path),'before':hashlib.sha256(original).hexdigest(),'after':hashlib.sha256(path.read_bytes()).hexdigest(),'removed':old,'replacement':new}
  (r/(name+'-edit.json')).write_text(json.dumps(proof,indent=2))
  rc=run(name+'-broken',selector);assert rc!=0,(name,'control did not fail')
 finally:path.write_bytes(original)
 assert path.read_bytes()==original
 rc=run(name+'-restored',selector);assert rc==0,(name,'restore failed')
 results.append({'name':name,'caught':True,'byteRestored':True,'restoredPass':True})
(r/'controls.json').write_text(json.dumps(results,indent=2));print(json.dumps(results))
