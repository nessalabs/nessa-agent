from pathlib import Path
import subprocess,hashlib,json,time
root=Path('/workspace/nessa-agent-bounded-persistence'); frozen=Path('/tmp/627-final-candidate-freeze'); out=Path('/tmp/627-boundary-mutations');out.mkdir(exist_ok=True)
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

mutate('queued-drop-uncontained-strengthened',[sdk],lambda:replace(sdk,'        let _ = self.drop_operation();\n        // Automatic','        drop(self.operation.take());\n        // Automatic'),'nessa-sdk','queued_abort_input_drop_fault_keeps_slot')
mutate('server-queued-drop-uncontained-strengthened',[server],lambda:replace(server,'        let _ = self.drop_operation();\n        // Automatic','        drop(self.operation.take());\n        // Automatic'),'nessa-server','queued_abort_input_drop_fault_keeps_slot')
mutate('ready-cleanup-fault-acknowledged',[sdk],lambda:replace(sdk,'let result = if cleaned {','let result = if cleaned || output.is_ok() {'),'nessa-sdk','ready_effect_input_drop_fault_is_uncertain')
mutate('server-ready-cleanup-fault-acknowledged',[server],lambda:replace(server,'let result = if cleaned {','let result = if cleaned || output.is_ok() {'),'nessa-server','ready_effect_input_drop_fault_is_uncertain')
mutate('server-call-uncontained',[server],lambda:replace(server,'let called = catch_unwind(AssertUnwindSafe(|| self.call_operation()));','let called: Result<R, Box<dyn std::any::Any + Send>> = Ok(self.call_operation());'),'nessa-server','detached_caller_faulting_payload_is_contained')
mutate('capture-drop-uncontained',[sdk],lambda:replace(sdk,'contained_drop(self.operation.take())','{ drop(self.operation.take()); true }'),'nessa-sdk','detached_ready_input_drop_fault_is_contained')
mutate('server-capture-drop-uncontained',[server],lambda:replace(server,'contained_drop(self.operation.take())','{ drop(self.operation.take()); true }'),'nessa-server','detached_ready_input_drop_fault_is_contained')
mutate('definite-sqlite-write',[own],lambda:replace(own,'.run(permit, move || state.write(snapshot.get()))\n            .await\n            .map_err(|_| PortFailure::Uncertain)?','.run(permit, move || state.write(snapshot.get()))\n            .await\n            .unwrap_or(Ok(()))'),'nessa-sdk','pre_effect_worker_panic_is_typed_and_landed_effect_is_uncertain')
mutate('secret-read-as-absence',[rec],lambda:replace(rec,'.run(permit, move || state.get().load_secret(server))\n            .await\n            .map_err(|_| RecordFailure::Unavailable)?','.run(permit, move || state.get().load_secret(server))\n            .await\n            .unwrap_or(Ok(None))'),'nessa-server','interrupted_record_operations_are_unavailable_or_unknown')
mutate('definite-record-store',[rec],lambda:replace(rec,'.run(permit, move || state.store(auth.get()))\n            .await\n            .map_err(|_| RecordFailure::Unavailable)?','.run(permit, move || state.store(auth.get()))\n            .await\n            .unwrap_or(Ok(()))'),'nessa-server','ready_effect_input_drop_fault_is_uncertain')
