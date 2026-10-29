from pathlib import Path
import json,subprocess,os,hashlib
probe=Path('/tmp/nessa-webhook-outbox-probes')
base=probe/'crates/nessa-server/src/webhooks/infrastructure/outbox'
args=json.loads((probe/'args.json').read_text());args[args.index('-o')+1]=str(probe/'contextual-mutation-bin')
env=dict(os.environ,RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
original={p:p.read_bytes() for p in base.iterdir() if p.is_file()}
rules=[]
def rule(file,old,new,test):
 rules.append((file,old,new,test))
reg='registration_issue_evidence_matches_retained_target'
stop='registration_stop_evidence_matches_original_stop_and_target'
ret='retirement_evidence_uses_retained_original_facts_after_private_row_deletion'
cur='cursor_evidence_matches_complete_original_reset_observation'
reset='reset_facts_survive_audit_trim_and_decode_completely'
deliv='delivery_evidence_matches_enclosing_facts_before_publication'
rule('restoration.rs','if audit::checked_target(credential, principal, organization, owner)? != *target','if false && audit::checked_target(credential, principal, organization, owner)? != *target',reg)
rule('restoration.rs','if *at_ms != issued_at','if false && *at_ms != issued_at',reg)
rule('restoration.rs','if stop != Some(&audit::check_stop(cause, initiator, *at_ms)?)','if false && stop != Some(&audit::check_stop(cause, initiator, *at_ms)?)',stop)
rule('restoration.rs','retired.audit != uuid(record_id)?','false',ret)
rule('restoration.rs','retired.at_ms != *at_ms','false',ret)
old='''retired.stop
                            != audit::check_stop(
                                stop_cause.as_ref().ok_or(OutboxError::Invalid)?,
                                initiator,
                                stop_at_ms.ok_or(OutboxError::Invalid)?,
                            )?'''
rule('restoration.rs',old,'false',ret)
for old in ['current.scope != prior_scope.checked()?','current.position != *prior_position','reset.observed_scope != observed_scope.checked()?','reset.stop.initiator != audit::check_reader(initiator, id)?','reset.stop.at_ms != *at_ms']:
 rule('restoration.rs',old,'false',cur)
for old in ['evidence.target != registration.registration.target','evidence.scope != saved.scope','evidence.position != saved.position']:
 rule('restoration.rs',old,'false',deliv)
rule('publication.rs','super::restoration::validate_owed(connection)?;','', 'complete_owed_batch_is_checked_before_first_publication')
rule('retirement.rs','if count > MAX_REGISTRATIONS','if false && count > MAX_REGISTRATIONS',ret)
rule('retirement.rs','cause: audit::Cause::Drained,','cause: _,',ret)
rule('journal.rs','connection.execute("DELETE FROM retirements WHERE NOT EXISTS(SELECT 1 FROM audit WHERE audit.registration=retirements.id)",[]).map_err(unavailable)?;','',ret)
rule('schema.sql',' REFERENCES audit(id) DEFERRABLE INITIALLY DEFERRED','',ret)
rule('cursor.rs','super::integer(stop.at_ms)?;','',reset)
rule('cursor.rs','_ => return Err(OutboxError::Invalid),\n        };\n        Ok(Cursor','_ => None,\n        };\n        Ok(Cursor',reset)
# Total branches must each matter to concrete cross-record refusal.
for marker,test in [('Record::Registration {',reg),('Record::Cursor {',cur),('Record::Delivery {',deliv)]:
 text=(base/'restoration.rs').read_text();a=text.index(marker);start=text.index('=> {',a)+3;i=start;depth=0
 while i<len(text):
  if text[i]=='{':depth+=1
  if text[i]=='}':
   depth-=1
   if depth==0:break
  i+=1
 rule('restoration.rs',text[start:i+1],'{}',test)
rule('restoration.rs','validate_histories(connection, true)','Ok(())','live_owed_delivery_history_matches_current_before_publication')
rule('restoration.rs','.map_or(next.before.is_some(), |prior| !next.follows(prior))','.map_or(false, |_prior| false)','retained_audit_chain_correlates_checkpoint_target_and_current_state')
rule('restoration.rs','last.after != saved.delivery','false','retained_audit_chain_correlates_checkpoint_target_and_current_state')
rule('restoration.rs','last.source_stop != saved.source_stop','false','settled_source_context_must_match_the_enclosing_delivery')
results=[]
try:
 for index,(file,old,new,test) in enumerate(rules):
  p=base/file;src=original[p].decode();assert src.count(old)==1,(index,file,old,src.count(old));p.write_text(src.replace(old,new))
  compiled=subprocess.run(args,env=env,text=True,capture_output=True)
  if compiled.returncode: raise RuntimeError(f'compile {index}: {compiled.stderr[-2000:]}')
  run=subprocess.run([str(probe/'contextual-mutation-bin'),test],env=env,text=True,capture_output=True,timeout=120)
  if run.returncode==0:raise RuntimeError(f'uncaught mutation {index} {file} {old}')
  results.append(dict(index=index,file=file,rule=old,test=test,mutationCaught=True))
  p.write_bytes(original[p])
  restored=subprocess.run(args,env=env,text=True,capture_output=True);assert restored.returncode==0,restored.stderr
  control=subprocess.run([str(probe/'contextual-mutation-bin'),test],env=env,text=True,capture_output=True,timeout=120)
  assert control.returncode==0,(index,control.stdout,control.stderr)
  results[-1]['restoredPassed']=True
  print(index,file,'caught/restored',flush=True)
finally:
 for p,data in original.items():p.write_bytes(data)
 (probe/'contextual-mutations.json').write_text(json.dumps(results,indent=2)+'\n')
assert all(p.read_bytes()==data for p,data in original.items())
print('all',len(results),'source copies restored byte-identical')
