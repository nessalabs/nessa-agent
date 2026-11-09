from pathlib import Path
import os,subprocess,json,shutil
root=Path('/workspace/nessa-webhook-runtime');probe=Path('/tmp/nessa-registration-probes')
for part in ['src/webhooks','tests/webhooks']:
 shutil.copytree(root/'crates/nessa-server'/part,probe/'crates/nessa-server'/part,dirs_exist_ok=True)
base=probe/'crates/nessa-server/src/webhooks'
args=json.loads((probe/'args.json').read_text());args[args.index('-o')+1]=str(probe/'mutated-bin')
env=os.environ|dict(RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
original={p:p.read_bytes() for p in base.rglob('*') if p.is_file()};rules=[]
def rule(file,old,new,test):rules.append((file,old,new,test))
app='application/registration.rs';store='infrastructure/outbox/intents.rs'
first='registration_commits_real_auth'
rule(app,'proposed.issue.purpose = CredentialIssuePurpose::Webhook;','',first)
rule(app,'proposed.issue.issued_at = proposed.observed_at_ms / 1000;','',first)
rule(app,'if self.issue.issued_at != self.observed_at_ms / 1000','if false && self.issue.issued_at != self.observed_at_ms / 1000','installed_intent_contradictions')
rule(app,'!matches_request(&intent.issue, metadata)','false','coherent_auth_result')
rule(app,'transitions[0].correlation.as_deref() != Some(&intent.issue.request_id)','false','contradictory_auth_reply')
rule(app,'CredentialTransition::try_from(transitions[0].clone()).as_ref() != Ok(&expected)','false','contradictory_auth_reply')
rule(app,'if !hold.snapshot.membership.is_active()','if false && !hold.snapshot.membership.is_active()','unavailable_or_substituted_current_authority')
rule(app,'if hold.snapshot.credential != credential || hold.snapshot.membership != membership','if false && (hold.snapshot.credential != credential || hold.snapshot.membership != membership)','unavailable_or_substituted_current_authority')
rule(store,'if count>=MAX_REGISTRATIONS as i64','if false && count>=MAX_REGISTRATIONS as i64','intent_count_and_byte_capacity')
rule(store,'if issue.len()>ISSUE_BYTES','if false && issue.len()>ISSUE_BYTES','intent_count_and_byte_capacity')
rule(store,'if same_intent(&saved,&proposed)','if true','conflicting_config_and_caller_inputs')
rule(store,'if !same_intent(&saved, &intent)','if false && !same_intent(&saved, &intent)','installation_requires_the_retained_original_intent')
rule(store,'if installed {','if false && installed {',first)
rule(store,'if !same_registration(&registration.registration, &intent.registration()?)','if false && !same_registration(&registration.registration, &intent.registration()?)','installed_intent_contradictions')
rule(store,'registration.issued_at_ms != intent.observed_at_ms','false','installed_intent_contradictions')
rule(store,'check_row_bytes(row, 6, ROW_BYTES)?;','','oversized_borrowed_row')
rule('infrastructure/outbox/schema.rs','check_row_bytes(row, 4, 2 * SCHEMA.len())?;','','schema_text_is_bounded')
rule('infrastructure/outbox/schema.rs','if definitions(connection, expected.len() as i64 + 1)? != expected','if false && definitions(connection, expected.len() as i64 + 1)? != expected','registration_schema_guards')
for name,test in [('registration_intent_immutable',first),('registration_intent_retire','registration_retirement_removes_private_intent')]:
 src=(base/'infrastructure/outbox/schema.sql').read_text();a=src.index('CREATE TRIGGER '+name+' ');b=src.index('END;',a)+4
 rule('infrastructure/outbox/schema.sql',src[a:b],'',test)
rule(app,'!hold\n                .snapshot\n                .credential\n                .is_valid_at(self.clock.unix_seconds())','false','replayed_current_revocation_and_expiry')
results=[]
def compile():
 r=subprocess.run(args,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 assert r.returncode==0,r.stdout.decode()[-3000:]
try:
 compile()
 baseline=subprocess.run([str(probe/'mutated-bin'),'registration','--test-threads=4'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 (probe/'baseline.txt').write_bytes(baseline.stdout);assert baseline.returncode==0,baseline.stdout.decode()
 for i,(file,old,new,test) in enumerate(rules):
  path=base/file;src=original[path].decode();assert src.count(old)==1,(i,file,old,src.count(old))
  path.write_text(src.replace(old,new));compile()
  r=subprocess.run([str(probe/'mutated-bin'),test],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
  (probe/f'{i}-mutated.txt').write_bytes(r.stdout)
  path.write_bytes(original[path]);compile()
  c=subprocess.run([str(probe/'mutated-bin'),test],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
  (probe/f'{i}-restored.txt').write_bytes(c.stdout)
  results.append(dict(index=i,file=file,rule=old,test=test,caught=r.returncode!=0,restored=c.returncode==0))
  (probe/'results.json').write_text(json.dumps(results,indent=2)+'\n')
  print(i,'caught' if r.returncode else 'NOT CAUGHT','restored' if c.returncode==0 else 'BAD CONTROL',flush=True)
finally:
 for path,data in original.items():path.write_bytes(data)
 print('all copied source restored:',all(p.read_bytes()==b for p,b in original.items()),flush=True)
