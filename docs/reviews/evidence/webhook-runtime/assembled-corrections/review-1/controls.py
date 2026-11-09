import json,subprocess,os,hashlib
from pathlib import Path
p=Path('/tmp/nessa-assembled-independent-review');env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
controls=[
 ('helper-actor','crates/nessa-server/src/webhooks/infrastructure/outbox/registry.rs','ExpiryObserver::Lifecycle => "lifecycle".into(),','ExpiryObserver::Lifecycle => format!("delivery:{}", uuid::Uuid::nil()),','automatic_expiry_observer_decoding',False),
 ('first-stop','crates/nessa-server/src/webhooks/infrastructure/outbox/registry.rs','if saved.stop.is_some() { return Ok(()); }','if false { return Ok(()); }','first_lifecycle_stop_survives',False),
 ('session-purpose','crates/nessa-auth/src/adapters/cedar/attributes.rs','crate::domain::IssuanceCause::WebhookIssue => "webhook_issue",','crate::domain::IssuanceCause::WebhookIssue => "admin_issue",','actual_webhook_origin_refuses_bearer',True),
 ('origin-revision','crates/nessa-auth/src/application/origin.rs','self.revision <= revision','true', 'matching_reply_cannot_turn_a_future_origin',True),
 ('materialized','crates/nessa-server/src/webhooks/infrastructure/outbox/journal.rs','WITH cutoffs AS MATERIALIZED (','WITH cutoffs AS (','terminal_cleanup_walks_one_indexed',False),
]
def compile(aut,label):
 for typ in (['auth-resumed-args'] if aut else [])+['args-resumed']:
  with (p/(label+'-'+typ+'-compile.txt')).open('w')as f:
   r=subprocess.run(json.loads((p/(typ+'.json')).read_text()),stdout=f,stderr=subprocess.STDOUT,env=env)
  assert r.returncode==0,(label,typ)
def run(label,selector):
 with(p/(label+'-tests.txt')).open('w')as f:
  r=subprocess.run([str(p/'tests-bin'),selector,'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env)
 s=(p/(label+'-tests.txt')).read_text();assert 'running 0 tests' not in s,(label,selector)
 return r.returncode
results=[]
for name,rel,old,new,selector,auth in controls:
 f=p/rel;original=f.read_bytes();s=original.decode();assert s.count(old)==1,(name,s.count(old));changed=s.replace(old,new);assert changed!=s
 (p/(name+'-hunk.json')).write_text(json.dumps({'path':rel,'old':old,'new':new},indent=2))
 try:
  f.write_text(changed);assert f.read_bytes()!=original;compile(auth,name+'-mutated');rc=run(name+'-mutated',selector);assert rc!=0,(name,'survived')
 finally:
  f.write_bytes(original);assert f.read_bytes()==original
 compile(auth,name+'-restored');assert run(name+'-restored',selector)==0,name
 results.append({'name':name,'caught':True,'restored_sha256':hashlib.sha256(original).hexdigest()});(p/'mutations.json').write_text(json.dumps(results,indent=2));print(name,'caught/restored',flush=True)
with(p/'final-108.txt').open('w')as f:subprocess.run([str(p/'tests-bin'),'--nocapture'],stdout=f,stderr=subprocess.STDOUT,env=env,check=True)
print((p/'final-108.txt').read_text()[-300:])
