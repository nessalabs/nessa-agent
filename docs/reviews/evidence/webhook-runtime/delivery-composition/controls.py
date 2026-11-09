import hashlib,json,os,pathlib,re,subprocess
root=pathlib.Path('/workspace/nessa-webhook-delivery')
paths={k:root/v for k,v in {
 'delivery':'crates/nessa-server/src/webhooks/infrastructure/runtime/delivery.rs',
 'runtime':'crates/nessa-server/src/webhooks/infrastructure/runtime.rs',
 'store':'crates/nessa-server/src/webhooks/infrastructure/outbox/store.rs',
}.items()}
baseline={key:path.read_bytes() for key,path in paths.items()}
env=os.environ.copy();env.update(CARGO_HOME='/workspace/nessa-toolchain/cargo',RUSTUP_HOME='/workspace/nessa-toolchain/rustup',CARGO_TARGET_DIR='/workspace/nessa-webhook-registry-order/target',CARGO_INCREMENTAL='0');env['PATH']='/workspace/nessa-toolchain/cargo/bin:'+env['PATH']
def once(text,old,new):
 assert text.count(old)==1,(old,text.count(old));return text.replace(old,new,1)
def batch(text):
 a=text.index('        for _ in 0..MAX_OUTBOX_WORKERS {',text.index('async fn delivery_cycle'));b=text.index('            self.send_pending(dependencies, pending).await?;',a)
 return text[:a]+'''        let due = self.owner.store.due(self.owner.clock.unix_milliseconds()).await.map_err(|_|CredentialAdminError::Unavailable)?;
        for pending in due {
            if !self.accepting_delivery() { break; }
'''+text[b:]
def no_ack(text):
 a=text.index('        self.owner\n            .store\n            .transition(&pending.delivery, &sending, DeliveryCause::Admission, at)');b=text.index('        if head.value().is_err() {',a)
 return text[:a]+'        if false {\n'+text[a:b]+'        }\n'+text[b:]
def no_context(text):
 a=text.index('fn response_committed(')
 return text[:a]+once(text[a:],'    restoration::validate(connection)?;\n','')
CASES=[
 ('redundant_context','store',no_context,'erased_delivery_or_response_evidence_never_establishes_acknowledgement',True),
 ('single_batch','delivery',batch,'webhooks::infrastructure::product_tests::delivery',True),
 ('sending_ack','delivery',no_ack,'missing_sending_ack_makes_no_post_and_cold_recovery_never_reposts_unknown',False),
 ('post_expiry','delivery',lambda s:once(s,'        if head.value().is_err() {','        if false {'),'expiry_during_actual_sending_commit_refuses_dispatch_after_acknowledgement',False),
 ('observation_capture','delivery',lambda s:once(s,'        let observed = observation(prepared.post(&pending.registration.key, &message).await);','        let observed = observation(prepared.post(&pending.registration.key, &message).await);\n        self.owner.clock.unix_milliseconds();'),'actual_response_is_owned_before_a_later_clock_callback_panics',False),
 ('physical_join','runtime',lambda s:once(s,'''        if let Some(delivery) = delivery {
            if !matches!(delivery.await, Ok(Ok(()))) {
                failed = true;
            }
        }''','        drop(delivery);'),'actual_post_holds_revocation_and_production_cleanup_joins_it_before_confirmation',False),
 ('response_time','store',lambda s:once(s,' && actual == at_ms',''),'response_reconciliation_requires_original_transition_and_time_before_effects',False),
 ('preparing','delivery',lambda s:once(s,' | RecordReadError::SourcePreparing',''),'a_preparing_source_pauses_without_failing_the_shared_command_owner_or_sending',False),
 ('retired_snapshot','delivery',lambda s:once(s,'Ok(()) | Err(OutboxError::Conflict) =>','Ok(()) =>'),'public_revoke_wins_during_preparation_without_post_or_failed_cleanup',False),
]
manifest=pathlib.Path('/tmp/nessa-896-delivery-controls.jsonl')
with manifest.open('w') as out:
 for name,key,change,test,may_pass in CASES:
  try:
   assert all(path.read_bytes()==baseline[k] for k,path in paths.items())
   mutated=change(baseline[key].decode());paths[key].write_text(mutated)
   print('START '+name,flush=True)
   log=pathlib.Path('/tmp/nessa-896-delivery-control-'+name+'.log')
   with log.open('w') as stream:
    result=subprocess.run(['cargo','test','--locked','-p','nessa-server','--lib',test,'--','--nocapture'],cwd=root,env=env,stdout=stream,stderr=subprocess.STDOUT)
   text=log.read_text();compiled=not re.search(r'error\[E\d+\]|could not compile',text)
   caught=result.returncode!=0 and compiled and 'test result: FAILED' in text
   valid=caught or (may_pass and result.returncode==0)
   record={'name':name,'test':test,'returncode':result.returncode,'runtime_failure':caught,'removable_if_passed':may_pass,'valid':valid,'baseline_sha256':hashlib.sha256(baseline[key]).hexdigest(),'mutated_sha256':hashlib.sha256(mutated.encode()).hexdigest()}
   out.write(json.dumps(record)+'\n');out.flush();print('END '+json.dumps(record),flush=True)
   if not valid:raise RuntimeError('control not qualified: '+name)
  finally:
   for k,path in paths.items():path.write_bytes(baseline[k])
assert all(path.read_bytes()==baseline[k] for k,path in paths.items())
print('RESTORED ALL SOURCE BYTES',flush=True)
