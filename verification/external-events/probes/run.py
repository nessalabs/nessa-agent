#!/usr/bin/env python3
"""Prove HTTP-boundary rules load-bearing without changing the shared source tree.

The copied HTTP adapter uses deterministic application-port substitutes here.
This supplements, and does not replace, the real ingress/SDK E2E tests.
Dependencies must already be in Cargo's cache (`--offline`).
"""
from pathlib import Path
from tempfile import TemporaryDirectory
import subprocess,json,hashlib,re,os
ROOT=Path(__file__).resolve().parents[3]
HERE=Path(__file__).resolve().parent
workspace=TemporaryDirectory(prefix='nessa-external-event-probes-')
crate=Path(workspace.name)
(crate/'src').mkdir()
(crate/'Cargo.toml').write_text((HERE/'Cargo.toml.template').read_text())
http_copy=crate/'src/http.rs'
identity=ROOT/'crates/nessa-server/src/external_events/domain/value_objects/identity.rs'
harness=(HERE/'harness.rs').read_text().replace('@IDENTITY_SOURCE@',identity.as_posix()).replace('@HTTP_COPY@',http_copy.as_posix())
(crate/'src/lib.rs').write_text(harness)
source=(ROOT/'crates/nessa-server/src/external_events/entrypoint/http.rs').read_text()
tests=(HERE/'http-tests.rs').read_text()
path=http_copy
cmd=['cargo','test','--offline','--manifest-path',str(crate/'Cargo.toml'),'--lib','probe_tests','--','--test-threads=1']
env=dict(os.environ,CARGO_TARGET_DIR=str(ROOT/'target/external-event-probes'),CARGO_BUILD_JOBS='2')
def run(name,code,expected):
 path.write_text(code+'\n'+tests);assert path.read_text().startswith(code)
 r=subprocess.run(cmd,env=env,cwd=ROOT,capture_output=True,text=True,timeout=600)
 (crate/('probe-'+name+'.log')).write_text(r.stdout+r.stderr)
 if expected=='pass':assert r.returncode==0,r.stdout+r.stderr
 else:assert r.returncode!=0 and 'test result: FAILED' in r.stdout,(name,r.stdout+r.stderr)
 return {'rule':name,'exit':r.returncode,'sourceSha256':hashlib.sha256(source.encode()).hexdigest(),'mutantCompiled':r.returncode==0 or 'test result: FAILED' in r.stdout,'failedTests':re.findall(r'^test (\S+) \.{3} FAILED$',r.stdout,re.M)}
results=[run('baseline',source,'pass')]
mutations=[
 ('hmac-verification','if mac.verify_slice(&signature).is_err()','if false'),
 ('freshness','abs_diff(seconds) > 300','abs_diff(seconds) > u64::MAX'),
 ('canonical-timestamp','seconds.to_string() != timestamp','false'),
 ('single-signature-headers','parts.headers.get_all(*name).iter().count() != 1','false'),
 ('strict-envelope','#[serde(deny_unknown_fields)]',''),
 ('body-byte-bound','to_bytes(body, MAX_EVENT_BYTES)','to_bytes(body, usize::MAX)'),
 ('body-deadline','Duration::from_secs(5)','Duration::from_secs(6)'),
 ('binding-signature-scope','mac.update(state.binding.id.as_str().as_bytes());','mac.update(b"ci");'),
 ('signature-decode-allocation','if signature.len() != 44 {','if false {'),
 ('handler-capacity','state.slots.clone().try_acquire_owned()','Arc::new(Semaphore::new(8)).try_acquire_owned()'),
 ('caller-loss-capacity','let admission = tokio::spawn(async move {\n        let _slot = slot;','let _slot = slot;\n    let admission = tokio::spawn(async move {'),
]
try:
 for name,before,after in mutations:
  assert before in source,name
  results.append(run(name,source.replace(before,after,1),'fail'))
 pattern=r'    if parts\n        \.headers\n.*?StatusCode::UNSUPPORTED_MEDIA_TYPE, "content_type"\);\n    \}\n'
 mutant,count=re.subn(pattern,'',source,count=1,flags=re.S);assert count==1
 results.append(run('content-type',mutant,'fail'))
finally:path.write_text(source+'\n'+tests)
(HERE.parent/'http-mutation-probes.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(results))
