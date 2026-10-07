import pathlib,subprocess,os,json,hashlib,time
root=pathlib.Path('/workspace/nessa-agent-bounded-persistence');src=root/'crates/nessa-local-storage/src/physical_operation.rs';original=src.read_bytes();out=pathlib.Path('/tmp/627-r1-submission-proof');env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup',CARGO_TARGET_DIR='/workspace/nessa-agent/target',CARGO_NET_OFFLINE='true',CARGO_BUILD_JOBS='2',CARGO_INCREMENTAL='0',PATH='/tmp/nessa-tools/bin:/tmp/nessa-cargo/bin:'+env['PATH'])
command=['/tmp/nessa-browser-libs/usr/bin/tini','-s','--','cargo','test','-p','nessa-local-storage','--features','physical-operation','--lib','submission_fault_payload_is_not_deferred_to_observer','--','--nocapture']
old='''        let submitted = catch_unwind(AssertUnwindSafe(|| self.spawn(operation))).map_err(|payload| {
            std::mem::forget(payload);
            Interrupted
        });'''
assert old in original.decode()
for name,new in [('entry-uncontained','        let submitted: Result<_, Interrupted> = Ok(self.spawn(operation));'),('entry-payload-dropped',old.replace('std::mem::forget(payload);','drop(payload);'))]:
 changed=original.decode().replace(old,new,1);src.write_text(changed);(out/(name+'.source')).write_text(changed)
 with (out/(name+'-red.log')).open('w') as f:r=subprocess.run(command,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT,timeout=120)
 src.write_bytes(original);os.utime(src,None)
 with (out/(name+'-restored.log')).open('w') as f:g=subprocess.run(command,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT,timeout=120)
 result={'mutation':name,'red_exit':r.returncode,'restored_exit':g.returncode,'restored_source_sha256':hashlib.sha256(src.read_bytes()).hexdigest(),'candidate_sha256':hashlib.sha256(original).hexdigest()};(out/(name+'-result.json')).write_text(json.dumps(result,indent=2)+'\n');print(result,flush=True)
 assert r.returncode==101 and g.returncode==0 and src.read_bytes()==original
