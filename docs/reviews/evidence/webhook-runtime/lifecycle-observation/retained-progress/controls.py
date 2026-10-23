import hashlib,json,os,subprocess
from pathlib import Path
r=Path('/tmp/nessa-lifecycle-frozen'); out=Path('/tmp/nessa-w24-A-controls');out.mkdir(exist_ok=True)
a=json.loads((r/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
p=r/'crates/nessa-server/src/webhooks/infrastructure/outbox/restoration/progress.rs'; restoration=p.parent.parent/'restoration.rs'; original=p.read_bytes();original_r=restoration.read_bytes()
def run(name,test):
 with(out/(name+'-compile.txt')).open('w')as f: subprocess.run(a,stdout=f,stderr=subprocess.STDOUT,env=env,check=True)
 with(out/(name+'-tests.txt')).open('w')as f:return subprocess.run([str(r/'tests-bin'),test,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env).returncode
# Compile the actual old reviewed owner with the new desired-contract tests.
# Keep the new module declared only so the query-plan tests compile; old code
# does not call it. No mutation of the real checkout or preserved review occurs.
old=subprocess.check_output(['git','show','4c993d457c0b1e5b3a05631fa09dcae21528737d:crates/nessa-server/src/webhooks/infrastructure/outbox/restoration.rs'],cwd='/workspace/nessa-webhook-composition').decode()
try:
 restoration.write_text(old+'\npub(super) mod progress;\n')
 for case,test in [('M3-reset-before','initial_rejection_with_only_retained_source_history'),('M3-identity-before','published_initial_identity_excludes_a_second_pending_identity')]:
  assert run(case,test)!=0,case
finally: restoration.write_bytes(original_r)
assert run('M3-after','issuance_context_tests')==0
cases=[
 ('identity','                .is_some()','                .is_some() && false','published_initial_identity_excludes_a_second_pending_identity'),
 ('pending','if matches!(progress.initial, Some((_, false)))','if false && matches!(progress.initial, Some((_, false)))','initial_rejection_with_only_retained_source_history'),
 ('phase','if registry::by_id(connection, registration)?.lifecycle.phase() != RegistrationPhase::Active','if registry::by_id(connection, registration)?.lifecycle.phase() == RegistrationPhase::Active','genuine_initial_issue_lost_publication_ack'),
 ('reset','EXISTS(SELECT 1 FROM source_reset_facts WHERE registration=?1)','0','unowned_reset_is_contradictory'),
 ('audit-index','FROM audit WHERE registration=?1','FROM audit NOT INDEXED WHERE registration=?1','retained_audit_summary_reads_only'),
 ('cursor-index','FROM cursors WHERE registration=?1','FROM cursors NOT INDEXED WHERE registration=?1','retained_progress_uses_registration_keys'),
 ('delivery-index','FROM deliveries WHERE registration=?1','FROM deliveries NOT INDEXED WHERE registration=?1','retained_progress_uses_registration_keys'),
 ('scope','FROM audit WHERE registration=?1','FROM audit WHERE ?1 IS NOT NULL','genuine_initial_issue_lost_publication_ack'),
]
results=[]
for name,before,after,test in cases:
 s=original.decode();assert s.count(before)==1,(name,s.count(before))
 try:
  p.write_text(s.replace(before,after));assert p.read_bytes()!=original
  (out/(name+'-edit.json')).write_text(json.dumps(dict(before=before,after=after,originalSHA256=hashlib.sha256(original).hexdigest(),mutatedSHA256=hashlib.sha256(p.read_bytes()).hexdigest()),indent=2)+'\n')
  assert run(name+'-broken',test)!=0,(name,'escaped')
 finally:p.write_bytes(original);assert p.read_bytes()==original
 assert run(name+'-restored',test)==0
 results.append(dict(rule=name,caught=True,byteRestored=True));(out/'results.json').write_text(json.dumps(results,indent=2)+'\n');print(name,flush=True)
