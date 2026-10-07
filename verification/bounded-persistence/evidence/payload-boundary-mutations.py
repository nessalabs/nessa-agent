from pathlib import Path
import subprocess,hashlib,json,time
root=Path('/workspace/nessa-agent-bounded-persistence'); frozen=Path('/tmp/627-final-candidate-freeze'); out=Path('/tmp/627-payload-boundary-mutations');out.mkdir(exist_ok=True)
sdk='crates/nessa-sdk/src/infrastructure/session_storage/ownership/blocking.rs'; server='crates/nessa-server/src/mcp_authorization/infrastructure/blocking.rs';own='crates/nessa-sdk/src/infrastructure/session_storage/ownership.rs';rec='crates/nessa-server/src/mcp_authorization/infrastructure/records.rs'
cmd=['/tmp/nessa-browser-libs/usr/bin/tini','-s','--','env','CARGO_HOME=/tmp/nessa-cargo','RUSTUP_HOME=/tmp/nessa-rustup','PATH=/tmp/nessa-cargo/bin:'+__import__('os').environ['PATH'],'CARGO_TARGET_DIR=/workspace/nessa-agent/target','CARGO_NET_OFFLINE=true','CARGO_BUILD_JOBS=2','cargo','test']
def replace(path,a,b):
 p=root/path;s=p.read_text();assert a in s,(path,a);p.write_text(s.replace(a,b,1))
def mutate(name,paths,change,package,test):
 for p in paths: (root/p).write_bytes((frozen/p).read_bytes())
 change()
 patch=subprocess.check_output(['git','diff','--no-index','--',str(frozen/paths[0]),str(root/paths[0])],cwd=root) if False else ''
 import difflib
 patch=''.join(''.join(difflib.unified_diff((frozen/p).read_text().splitlines(True),(root/p).read_text().splitlines(True),fromfile='a/'+p,tofile='b/'+p)) for p in paths)
 (out/(name+'.patch')).write_text(patch)
 hashes={p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths};(out/(name+'.sha256.json')).write_text(json.dumps(hashes,indent=2)+'\n')
 with (out/(name+'.log')).open('w') as log:r=subprocess.run(cmd+['-p',package,'--lib',test,'--','--nocapture'],cwd=root,stdout=log,stderr=subprocess.STDOUT)
 if test in ['detached_caller_faulting_payload_is_contained','queued_abort_input_drop_fault_keeps_slot','detached_ready_input_drop_fault_is_contained','ready_effect_input_drop_fault_is_uncertain']:
  child_env=dict(__import__('os').environ)
  child_env['NESSA_627_FAULT_CASE' if package=='nessa-sdk' else 'NESSA_627_RECORD_FAULT_CASE']=test
  with (out/(name+'-child.log')).open('w') as log:subprocess.run(cmd+['-p',package,'--lib',test,'--','--nocapture'],cwd=root,env=child_env,stdout=log,stderr=subprocess.STDOUT)
 for p in paths: (root/p).write_bytes((frozen/p).read_bytes())
 with (out/(name+'-restored.log')).open('w') as log:g=subprocess.run(cmd+['-p',package,'--lib',test],cwd=root,stdout=log,stderr=subprocess.STDOUT)
 (out/(name+'.result.json')).write_text(json.dumps({'mutated_exit':r.returncode,'restored_exit':g.returncode,'restored_sha256':{p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths}},indent=2)+'\n')
 print(name,r.returncode,g.returncode,flush=True)
 assert r.returncode!=0 and g.returncode==0,name
 assert "test result: FAILED" in (out/(name+".log")).read_text(), "compile-only rejection: "+name


mutate('capture-payload-disposal',[sdk],lambda:replace(sdk,'Err(payload) => {\n            std::mem::forget(payload);\n            false','Err(payload) => {\n            drop(payload);\n            false'),'nessa-sdk','detached_ready_input_drop_fault_is_contained')
mutate('server-capture-payload-disposal',[server],lambda:replace(server,'Err(payload) => {\n            std::mem::forget(payload);\n            false','Err(payload) => {\n            drop(payload);\n            false'),'nessa-server','detached_ready_input_drop_fault_is_contained')
