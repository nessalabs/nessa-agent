from pathlib import Path
import shutil,json,subprocess,os
p=Path('/tmp/nessa-assembled-independent-review')
# Rerun existing independent cost probes against the newly assembled actual source.
s=Path('/tmp/nessa-retention-independent-review/probes.rs').read_text().replace('StopReason::Revoked{initiator:PrincipalId::new("synthetic-review-revoker").unwrap()}','StopReason::Expired{observed_by:ExpiryObserver::Lifecycle}').replace('a["initiator"]=="synthetic-review-revoker"','a["initiator"]=="lifecycle"')
(p/'cost-combined-probes.rs').write_text(s)
f=p/'crates/nessa-server/tests/webhooks/infrastructure/outbox.rs';f.write_text(f.read_text()+'\n#[path="/tmp/nessa-assembled-independent-review/cost-combined-probes.rs"] mod independent_cost_probes;\n')
f=p/'crates/nessa-server/tests/webhooks/infrastructure/outbox/registration.rs';f.write_text(f.read_text()+'\n#[path="/tmp/nessa-assembled-independent-review/combined-probes.rs"] mod independent_combined_probes;\n')
env=dict(os.environ,CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
with(p/'probes-compile.txt').open('w')as f:subprocess.run(json.loads((p/'args-resumed.json').read_text()),stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
with(p/'probes.txt').open('w')as f:subprocess.run([str(p/'tests-bin'),'independent','--nocapture'],stdout=f,stderr=subprocess.STDOUT,check=True,env=env)
print((p/'probes.txt').read_text()[-600:])
