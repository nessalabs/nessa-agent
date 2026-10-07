from pathlib import Path
import subprocess,hashlib,json,time
root=Path('/workspace/nessa-agent-bounded-persistence'); frozen=Path('/tmp/627-candidate-freeze'); out=Path('/tmp/627-mutations');out.mkdir(exist_ok=True)
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
 for p in paths: (root/p).write_bytes((frozen/p).read_bytes())
 with (out/(name+'-restored.log')).open('w') as log:g=subprocess.run(cmd+['-p',package,'--lib',test],cwd=root,stdout=log,stderr=subprocess.STDOUT)
 (out/(name+'.result.json')).write_text(json.dumps({'mutated_exit':r.returncode,'restored_exit':g.returncode,'restored_sha256':{p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths}},indent=2)+'\n')
 print(name,r.returncode,g.returncode,flush=True)
 assert r.returncode!=0 and g.returncode==0,name
 assert "test result: FAILED" in (out/(name+".log")).read_text(), "compile-only rejection: "+name
mutate('payload-disposal',[sdk],lambda:replace(sdk,'let output = called.map_err(|payload| {\n            std::mem::forget(payload);','let output = called.map_err(|payload| {\n            drop(payload);'),'nessa-sdk','detached_caller_faulting_payload_is_contained')
mutate('early-permit-release',[sdk],lambda: (replace(sdk,'_permit: admission.permit,','_permit: (),'),replace(sdk,'let job = Job {','let _waiter_permit = admission.permit;\n        let job = Job {'),replace(sdk,'_permit: OwnedSemaphorePermit,','_permit: (),')),'nessa-sdk','canceled_sqlite_write_excludes_following_io')
mutate('two-physical-slots',[sdk],lambda:replace(sdk,'PHYSICAL_SLOTS: usize = 1','PHYSICAL_SLOTS: usize = 2'),'nessa-sdk','canceled_sqlite_write_excludes_following_io')
def late():
 replace(own,'async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {','async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {\n        let snapshot = self.worker.input(snapshot.clone());')
 replace(own,'        let snapshot = self.worker.input(snapshot.clone());\n        self.worker','        self.worker')
mutate('input-before-admission',[own],late,'nessa-sdk','admission_precedes_owned_input_transfer')
mutate('ambient-executor',[sdk],lambda:replace(sdk,'admission.executor.spawn_blocking(move || job.run())','tokio::task::spawn_blocking(move || job.run())'),'nessa-sdk','queued_send_future_resumes_on_plain_thread')
mutate('definite-secret-delete',[rec],lambda:replace(rec,'.unwrap_or(Ok(Deletion::Unknown))','.unwrap_or(Ok(Deletion::Deleted))'),'nessa-server','ready_effect_input_drop_fault_is_uncertain')
mutate('server-payload-disposal',[server],lambda:replace(server,'let output = called.map_err(|payload| {\n            std::mem::forget(payload);','let output = called.map_err(|payload| {\n            drop(payload);'),'nessa-server','detached_caller_faulting_payload_is_contained')
mutate('server-early-permit-release',[server],lambda:(replace(server,'_permit: admission.permit,','_permit: (),'),replace(server,'let job = Job {','let _waiter_permit = admission.permit;\n        let job = Job {'),replace(server,'_permit: OwnedSemaphorePermit,','_permit: (),')),'nessa-server','canceled_secret_store_excludes_delete_and_load')
mutate('server-ambient-executor',[server],lambda:replace(server,'admission.executor.spawn_blocking(move || job.run())','tokio::task::spawn_blocking(move || job.run())'),'nessa-server','queued_send_future_resumes_on_plain_thread')
mutate('sqlite-read-as-absence',[own],lambda:replace(own,'.run(permit, move || state.get().read())\n            .await\n            .map_err(|_| PortFailure::Uncertain)?','.run(permit, move || state.get().read())\n            .await\n            .unwrap_or(Ok(OwnershipSnapshot::default()))'),'nessa-sdk','pre_effect_worker_panic_is_typed_and_landed_effect_is_uncertain')
mutate('record-read-as-absence',[rec],lambda:replace(rec,'.run(permit, move || state.get().load(server))\n            .await\n            .map_err(|_| RecordFailure::Unavailable)?','.run(permit, move || state.get().load(server))\n            .await\n            .unwrap_or(Ok(None))'),'nessa-server','ready_effect_input_drop_fault_is_uncertain')
mutate('definite-secret-publication',[rec],lambda:replace(rec,'.unwrap_or(Ok(Publication::Unknown))','.unwrap_or(Ok(Publication::Acknowledged))'),'nessa-server','ready_effect_input_drop_fault_is_uncertain')
audit='crates/nessa-server/src/mcp_authorization/infrastructure/audit.rs'
mutate('definite-audit-success',[audit],lambda:replace(audit,'.run(permit, move || state.record(record.get()))\n            .await\n            .map_err(|_| AuditFailure)?','.run(permit, move || state.record(record.get()))\n            .await\n            .unwrap_or(Ok(()))'),'nessa-server','interrupted_audit_and_poison_are_failures')
mutate('call-uncontained',[sdk],lambda:replace(sdk,'let called = catch_unwind(AssertUnwindSafe(|| self.call_operation()));','let called: Result<R, Box<dyn std::any::Any + Send>> = Ok(self.call_operation());'),'nessa-sdk','detached_caller_faulting_payload_is_contained')
mutate('queued-drop-uncontained',[sdk],lambda:replace(sdk,'        let _ = self.drop_operation();\n        // Automatic','        drop(self.operation.take());\n        // Automatic'),'nessa-sdk','queued_abort_input_drop_fault_keeps_slot')
mutate('ready-cleanup-fault-acknowledged',[sdk],lambda:replace(sdk,'let result = if cleaned {','let result = if cleaned || output.is_ok() {'),'nessa-sdk','ready_effect_input_drop_fault_is_uncertain')
