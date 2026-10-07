from pathlib import Path
import subprocess,difflib,json,hashlib,os
root=Path('/workspace/nessa-agent-bounded-persistence');frozen=Path('/tmp/627-final-candidate-freeze');out=Path('/tmp/627-inline-baseline');out.mkdir(exist_ok=True)
paths=['crates/nessa-sdk/src/infrastructure/session_storage/ownership.rs','crates/nessa-server/src/mcp_authorization/infrastructure/records.rs','crates/nessa-server/src/mcp_authorization/infrastructure/audit.rs']
blocks=['''#[async_trait]
impl OwnershipStore for SqliteOwnershipStore {
 async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> { self.state.write(snapshot) }
 async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> { self.state.read() }
}
''','''#[async_trait]
impl AuthorizationRecords for FileRecords {
 async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> { self.state.load(server) }
 async fn store(&self, auth: &ServerAuth) -> Result<(), RecordFailure> { self.state.store(auth) }
 async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> { self.state.load_secret(server) }
 async fn store_secret(&self, server: Uuid, secret: &TokenMaterial) -> Result<Publication, RecordFailure> { self.state.store_secret(server, secret) }
 async fn delete_secret(&self, server: Uuid) -> Result<Deletion, RecordFailure> { self.state.delete_secret(server) }
}
''','''#[async_trait]
impl AuthorizationAudit for FileAuthorizationAudit {
 async fn record(&self, record: &AuthAuditRecord) -> Result<(), AuditFailure> { self.state.record(record) }
}
''']
for p,b,state in zip(paths,blocks,['OwnershipState','FileRecordState','AuditState']):
 s=(frozen/p).read_text();start=s.index('#[async_trait]');end=s.index('\nimpl '+state,start);(root/p).write_text(s[:start]+b+s[end:])
(out/'inline.patch').write_text(''.join(''.join(difflib.unified_diff((frozen/p).read_text().splitlines(True),(root/p).read_text().splitlines(True),fromfile='a/'+p,tofile='b/'+p)) for p in paths))
(out/'sha256.json').write_text(json.dumps({p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths},indent=2)+'\n')
base=['/tmp/nessa-browser-libs/usr/bin/tini','-s','--','env','CARGO_HOME=/tmp/nessa-cargo','RUSTUP_HOME=/tmp/nessa-rustup','PATH=/tmp/nessa-cargo/bin:'+os.environ['PATH'],'CARGO_TARGET_DIR=/workspace/nessa-agent/target','CARGO_NET_OFFLINE=true','CARGO_BUILD_JOBS=2']
def run(label,args,bench=None):
 with (out/(label+'.log')).open('w') as log:r=subprocess.run(base+([] if bench is None else ['NESSA_627_BENCH_SAMPLES=1000','NESSA_627_BENCH_OUT=/tmp/627-'+bench+'.csv'])+['cargo','test']+args,cwd=root,stdout=log,stderr=subprocess.STDOUT)
 print(label,r.returncode,flush=True);return r.returncode
r=run('inline-heartbeat-red',['-p','nessa-sdk','-p','nessa-server','--lib','keeps_current_thread_heartbeat_running','--','--nocapture'])
assert r!=0 and 'test result: FAILED' in (out/'inline-heartbeat-red.log').read_text()
r=run('inline-server-heartbeat-red',['-p','nessa-server','--lib','keeps_current_thread_heartbeat_running','--','--nocapture']);assert r!=0
assert run('sqlite-inline',['-p','nessa-sdk','--test','infrastructure','benchmark_single_store_root_rows','--','--ignored'],'sqlite-inline')==0
assert run('oauth-inline',['-p','nessa-server','--test','mcp_authorization_benchmark','benchmark_scripted_authorize_callback_refresh','--','--ignored'],'oauth-inline')==0
for p in paths:(root/p).write_bytes((frozen/p).read_bytes())
assert run('restored-heartbeat-green',['-p','nessa-sdk','-p','nessa-server','--lib','keeps_current_thread_heartbeat_running','--','--nocapture'])==0
assert run('sqlite-worker-repeat',['-p','nessa-sdk','--test','infrastructure','benchmark_single_store_root_rows','--','--ignored'],'sqlite-worker-repeat')==0
assert run('oauth-worker-repeat',['-p','nessa-server','--test','mcp_authorization_benchmark','benchmark_scripted_authorize_callback_refresh','--','--ignored'],'oauth-worker-repeat')==0
