from pathlib import Path
import json,subprocess,shutil,os,hashlib
root=Path('/workspace/nessa-webhook-retention');h=Path('/tmp/nessa-retention-cost-controls');h.mkdir(exist_ok=True)
for rel in ['crates/nessa-server/src/webhooks','crates/nessa-server/tests/webhooks']:shutil.copytree(root/rel,h/rel,dirs_exist_ok=True)
(h/'lib.rs').write_text(Path('/tmp/nessa-origin-probes/lib.rs').read_text().replace('/tmp/nessa-origin-probes',str(h)))
a=json.loads(Path('/tmp/nessa-retention-cost-probes/args.json').read_text());a=[v.replace('/tmp/nessa-retention-cost-probes',str(h)) for v in a];(h/'args.json').write_text(json.dumps(a,indent=2));env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
def compile_run(label,test):
 with (h/(label+'-compile.txt')).open('w') as out:rc=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
 assert rc==0,(label,'compile')
 with (h/(label+'-test.txt')).open('w') as out:return subprocess.run([str(h/'tests-bin'),test],env=env,stdout=out,stderr=subprocess.STDOUT).returncode
assert compile_run('baseline','cleanup_tests')==0
journal='crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs';schema='crates/nessa-server/src/webhooks/infrastructure/outbox/schema.sql'
ranking="DELETE FROM deliveries WHERE id IN (SELECT id FROM (SELECT d.id,row_number() OVER (PARTITION BY registration ORDER BY d.rowid DESC) AS retained FROM deliveries d WHERE phase NOT IN ('queued','sending','retry')) WHERE retained>?1+1) AND NOT EXISTS (SELECT 1 FROM audit a WHERE a.delivery=deliveries.id AND a.published=0)"
def old_ranking(s):
 start=s.index('pub(super) const TRIM_TERMINALS: &str = ');a=s.index('"',start);b=s.index('";',a+1);return s[:a]+'"'+ranking+'"'+s[b+1:]
def replace(old,new):
 def edit(s):
  assert s.count(old)==1,(old,s.count(old));return s.replace(old,new,1)
 return edit
trigger="CREATE TRIGGER delivery_audit_retire AFTER DELETE ON deliveries\nWHEN OLD.phase NOT IN ('queued','sending','retry')\nBEGIN DELETE FROM audit WHERE delivery=OLD.id AND published=1; END;"
controls=[
 ('old-ranking',journal,old_ranking,'terminal_cleanup_walks_one_indexed_cutoff'),
 ('old-history-scan',journal,replace('DELETE FROM audit INDEXED BY published_registration_audit WHERE published=1 AND delivery IS NULL','DELETE FROM audit WHERE published=1 AND (delivery IS NULL OR NOT EXISTS(SELECT 1 FROM deliveries d WHERE d.id=audit.delivery))'),'registration_cleanup_does_not_visit'),
 ('owed-retention',journal,replace('AND NOT EXISTS (SELECT 1 FROM audit a WHERE a.delivery=deliveries.id AND a.published=0)',''),'delivery_retirement_removes_only'),
 ('delivery-history-retire',schema,replace(trigger,''),'delivery_retirement_removes_only'),
 ('published-only-retire',schema,replace('WHERE delivery=OLD.id AND published=1','WHERE delivery=OLD.id'),'delivery_retirement_removes_only'),
 ('terminal-only-retire',schema,replace("WHEN OLD.phase NOT IN ('queued','sending','retry')\n",''),'deleting_nonterminal_metadata'),
 ('materialized-cutoff',journal,replace('cutoffs AS MATERIALIZED','cutoffs AS'),'terminal_cleanup_walks_one_indexed_cutoff'),
 ('terminal-index',journal,lambda s:s.replace('INDEXED BY terminal_deliveries','NOT INDEXED'),'terminal_cleanup_walks_one_indexed_cutoff'),
 ('registration-index',journal,replace('INDEXED BY published_registration_audit','NOT INDEXED'),'registration_cleanup_does_not_visit'),
]
results=[]
for label,rel,edit,test in controls:
 p=h/rel;original=p.read_bytes()
 try:
  p.write_text(edit(original.decode()));caught=compile_run(label+'-mutated',test)!=0
 finally:p.write_bytes(original)
 restored=compile_run(label+'-restored',test)==0
 results.append(dict(rule=label,caught=caught,restored=restored,byte_identical=p.read_bytes()==(root/rel).read_bytes()))
 (h/'results.json').write_text(json.dumps(results,indent=2)+'\n')
 print(label,caught,restored,flush=True)
 if not caught or not restored:raise RuntimeError('uncaught or broken control '+label)
assert compile_run('final-restored','')==0
