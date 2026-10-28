from pathlib import Path
import json,subprocess,os
probe=Path('/tmp/nessa-webhook-outbox-probes');base=probe/'crates/nessa-server/src/webhooks/infrastructure/outbox'
args=json.loads((probe/'args.json').read_text());args[args.index('-o')+1]=str(probe/'shared-reset-mutation-bin')
env=dict(os.environ,RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
original={p:p.read_bytes() for p in base.rglob('*') if p.is_file()};rules=[]
def rule(file,old,new,test):rules.append((file,old,new,test))
immutable='sending_and_cursor_share_original_immutable_reset_and_membership'
ref='missing_references_and_reference_accounting_refuse_restore_before_effects'
life='reset_references_release_with_retirement_and_lost_ack_recovery'
fact='reset_fact_decoder_rejects_noncanonical_or_invalid_original_values'
meta='reset_metadata_substitutions_reject_before_recovery_publication'
for name in ['source_reset_fact_immutable','source_reset_member_immutable','source_reset_member_retention']:
 src=(base/'schema.sql').read_text();a=src.index('CREATE TRIGGER '+name+' ');b=src.index('END;',a)+4
 rule('schema.sql',src[a:b],'',immutable)
rule('schema.sql','WHEN NEW.refs=0','WHEN 0',life)
rule('source_reset/retention.rs','if definitions(connection, expected.len() as i64 + 1)? != expected','if false && definitions(connection, expected.len() as i64 + 1)? != expected','missing_or_replaced_immutable_guards_are_not_the_current_schema')
rule('source_reset/retention.rs','if refs <= 0 || actual != refs','if false && (refs <= 0 || actual != refs)',ref)
rule('source_reset/retention.rs','if count >= MAX_SOURCE_RESETS as i64','if false && count >= MAX_SOURCE_RESETS as i64','reset_factory_count_capacity_does_not_insert_a_partial_fact')
rule('capacity.rs','if count >= MAX_CURSORS as i64','if false && count >= MAX_CURSORS as i64','cursor_capacity_refuses_new_chat_without_partial_queue_or_reset')
rule('capacity.rs','GROUP BY registration HAVING count(*)>?2','GROUP BY registration HAVING 0>?2','cursor_capacity_refuses_new_chat_without_partial_queue_or_reset')
rule('source_reset/mod.rs','CASE WHEN bytes<=?2 THEN fact ELSE NULL END','CASE WHEN ?2>=0 THEN fact ELSE NULL END','oversized_fact_is_not_materialized_before_typed_decoding')
rule('source_reset/fact.rs','integer(self.stop.at_ms)?;','',fact)
rule('source_reset/fact.rs','integer(self.prior_position)?;','',fact)
rule('source_reset/fact.rs','if fact.encode()? != text','if false && fact.encode()? != text',fact)
rule('source_reset/mod.rs','fact.id != id','false',meta)
for condition in ['fact.stop.initiator.label(fact.registration) != *initiator','fact.stop.at_ms != *at_ms','fact.observed_scope != observed_scope.checked()?','fact.prior_position != *prior_position']:
 rule('restoration.rs',condition,'false',meta)
rule('restoration.rs','if next.source_reset.is_some() && !capture_seen','if false && next.source_reset.is_some() && !capture_seen','history_cannot_claim_reset_before_the_original_capture_state')
rule('restoration.rs','if *at_ms != capture.fact.stop.at_ms','if false && *at_ms != capture.fact.stop.at_ms','immediate_source_refusal_uses_original_reset_actor_time_and_identity')
rule('restoration.rs','"SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE published=0"','"SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE (?1=0 OR published=0)"','owed_selection_work_does_not_grow_with_published_history')
# Force a history-dependent OR with a bound literal instead of an unbound test parameter.
rules[-1]=(rules[-1][0],rules[-1][1],rules[-1][2].replace('?1=0','1=0'),rules[-1][3])
rule('source_reset/mod.rs','_ => OutboxError::Unavailable,','_ => OutboxError::Conflict,','capture_storage_failure_remains_unavailable_instead_of_conflict')
for condition in ['before.id() != current.id()','before.epoch() != current.epoch()','u64::try_from(prior_position).map_err(invalid)? != position','before.attempts() == current.attempts()','before.timestamp_ms() == current.timestamp_ms()']:
 replacement = 'true' if ' == ' in condition else 'false'
 rule('source_reset/mod.rs',condition,replacement,'missing_references_and_reference_accounting_refuse_restore_before_effects')
results=[]
try:
 for index,(file,old,new,test) in enumerate(rules):
  if index<int(os.environ.get('PROBE_START','0')):continue
  p=base/file;src=original[p].decode();assert src.count(old)==1,(index,file,old,src.count(old));p.write_text(src.replace(old,new))
  compiled=subprocess.run(args,env=env,text=True,capture_output=True)
  if compiled.returncode:raise RuntimeError(f'compile {index}: {compiled.stderr[-2500:]}')
  run=subprocess.run([str(probe/'shared-reset-mutation-bin'),test],env=env,text=True,capture_output=True,timeout=120)
  (probe/f'shared-reset-mutation-{index}.txt').write_text(run.stdout+run.stderr)
  if run.returncode==0:raise RuntimeError(f'uncaught mutation {index}: {file} {old}')
  p.write_bytes(original[p]);restored=subprocess.run(args,env=env,text=True,capture_output=True);assert restored.returncode==0,restored.stderr
  control=subprocess.run([str(probe/'shared-reset-mutation-bin'),test],env=env,text=True,capture_output=True,timeout=120)
  assert control.returncode==0,(index,control.stdout,control.stderr)
  results.append(dict(index=index,file=file,rule=old,test=test,mutationCaught=True,restoredPassed=True))
  print(index,file,'caught/restored',flush=True)
finally:
 for p,data in original.items():p.write_bytes(data)
 (probe/'shared-reset-mutations.json').write_text(json.dumps(results,indent=2)+'\n')
assert all(p.read_bytes()==data for p,data in original.items())
print('all',len(results),'source copies restored byte-identical',flush=True)
