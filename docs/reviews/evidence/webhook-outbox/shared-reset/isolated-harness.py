from pathlib import Path
import shutil,subprocess,os,json
root=Path('/workspace/nessa-agent')
probe=Path('/tmp/nessa-webhook-outbox-probes')
probe.mkdir(exist_ok=True)
for sub in ['src/webhooks','tests/webhooks']:
 shutil.copytree(root/'crates/nessa-server'/sub,probe/'crates/nessa-server'/sub,dirs_exist_ok=True)
(probe/'lib.rs').write_text('''#![allow(dead_code)]
mod webhooks {
 pub mod domain {
  #[path="/tmp/nessa-webhook-outbox-probes/crates/nessa-server/src/webhooks/domain/value_objects/delivery.rs"] mod delivery;
  #[path="/tmp/nessa-webhook-outbox-probes/crates/nessa-server/src/webhooks/domain/value_objects/destination.rs"] mod destination;
  pub use delivery::*; pub use destination::*;
 }
 pub mod application {
  use nessa_auth::domain::{CredentialId,OrganizationId,PrincipalId};
  #[derive(Clone, PartialEq, Eq)] pub struct Target {pub credential:CredentialId,pub principal:PrincipalId,pub organization:OrganizationId,pub owner:PrincipalId}
  #[path="/tmp/nessa-webhook-outbox-probes/crates/nessa-server/src/webhooks/application/message.rs"] mod message;
  pub use message::*;
  #[path="/tmp/nessa-webhook-outbox-probes/crates/nessa-server/src/webhooks/application/outbox.rs"] mod outbox;
  pub use outbox::*;
 }
 pub mod infrastructure {
  #[path="/tmp/nessa-webhook-outbox-probes/crates/nessa-server/src/webhooks/infrastructure/outbox/mod.rs"] pub mod outbox;
 }
}
''')
deps=root/'target/debug/deps'; fingerprints=deps.parent/'.fingerprint'
metadata=json.loads((fingerprints/'nessa-server-fe0ac38a8f68e2b7/test-lib-nessa_server.json').read_text())
wanted={d[1]:d[3] for d in metadata['deps']}
args=['/tmp/nessa-cargo/bin/rustc','--edition=2021','--test',str(probe/'lib.rs'),'-L',f'dependency={deps}','-C','debuginfo=0','-o',str(probe/'tests-bin')]
for crate in ['async_trait','nessa_auth','nessa_protocol','nessa_sync','nessa_local_database','nessa_local_storage','tokio','serde','serde_json','uuid','url','fs2','tempfile']:
 matched=[p for p in fingerprints.glob('*/lib-'+crate) if int.from_bytes(bytes.fromhex(p.read_text()),'little')==wanted[crate]]
 assert len(matched)==1,(crate,matched)
 suffix=matched[0].parent.name.rsplit('-',1)[1]
 matches=list(deps.glob(f'lib{crate}-{suffix}.rlib')) or list(deps.glob(f'lib{crate}-{suffix}.so'))
 assert len(matches)==1,(crate,matches)
 args+=['--extern',f'{crate}={matches[0]}']
(probe/'args.json').write_text(json.dumps(args))
env=dict(os.environ,RUSTUP_HOME='/tmp/nessa-rustup',CARGO_HOME='/tmp/nessa-cargo')
result=subprocess.run(args,env=env,text=True,capture_output=True)
(probe/'compile.txt').write_text(result.stdout+result.stderr)
print('compile',result.returncode)
if result.returncode:
 print(result.stderr[-3500:]);raise SystemExit(1)
if os.environ.get('PROBE_SKIP_RUN')=='1': raise SystemExit(0)
result=subprocess.run([str(probe/'tests-bin'),'webhooks'],env=env,text=True,capture_output=True,timeout=120)
(probe/'baseline.txt').write_text(result.stdout+result.stderr)
print(result.stdout[-4000:]);print('baseline',result.returncode)
raise SystemExit(result.returncode)
