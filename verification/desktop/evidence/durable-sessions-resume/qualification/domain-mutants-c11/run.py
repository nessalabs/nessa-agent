import hashlib,json,pathlib,subprocess,difflib,os
root=pathlib.Path('/workspace/nessa-agent')
out=pathlib.Path('/workspace/scratch/durable-qualification/domain-mutants-c11')
source=root/'crates/nessa-sdk/src/domain/agent_execution/sessions/value_objects/attachment.rs'
tests=root/'crates/nessa-sdk/tests/domain/agent_execution/provider_context.rs'
expected_head='6d1b413b19d2af4355fdf331778fe98fa4a683a4'
head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
assert head==expected_head,head
pristine=source.read_bytes(); testbytes=tests.read_bytes()
sha=lambda b:hashlib.sha256(b).hexdigest()
(out/'attachment.pristine.rs').write_bytes(pristine)
(out/'provider_context.rs').write_bytes(testbytes)
identity_origin=root/'target/debug/deps/libnessa_sdk-b4680c98ea90b2e2.rlib'
identity_bytes=identity_origin.read_bytes()
rlib=out/'libcurrent_sdk.rlib'
rlib.write_bytes(identity_bytes)
assert rlib.read_bytes()==identity_bytes

manifest={'source_head':head,'source':str(source),'source_sha256':sha(pristine),'tests':str(tests),'tests_sha256':sha(testbytes),'identity_rlib_origin':str(identity_origin),'identity_rlib':str(rlib),'identity_rlib_sha256':sha(rlib.read_bytes()),'rustc_version':subprocess.check_output(['rustc','--version'],text=True).strip(),'results':[]}
(out/'tmp').mkdir(exist_ok=True)
os.environ['TMPDIR']=str(out/'tmp')
(out/'harness.rs').write_text('''extern crate self as nessa_sdk;
extern crate current_sdk;
pub mod domain { pub mod agent_execution { pub mod sessions {
    pub use current_sdk::domain::agent_execution::sessions::ExecutionSessionId;
    #[path = "'''+str(out/'attachment.rs')+'''"] mod attachment;
    pub use attachment::*;
}}}
#[path = "'''+str(out/'provider_context.rs')+'''"] mod provider_context;
''')
def run(name,body,expected_failure=None):
    (out/'attachment.rs').write_bytes(pristine)
    assert sha((out/'attachment.rs').read_bytes())==manifest['source_sha256']
    (out/'attachment.rs').write_text(body)
    assert (out/'attachment.rs').read_text()==body
    diff=''.join(difflib.unified_diff(pristine.decode().splitlines(True),body.splitlines(True),fromfile='pristine/attachment.rs',tofile=name+'/attachment.rs'))
    (out/(name+'.diff')).write_text(diff)
    if expected_failure:
        assert diff and sha(body.encode()) != manifest['source_sha256']
    cmd=['rustc','--edition=2021','--test',str(out/'harness.rs'),'--crate-name','durable_domain_'+name,'--extern','current_sdk='+str(rlib),'-L','dependency='+str(root/'target/debug/deps'),'-o',str(out/(name+'.test'))]
    compile_result=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    (out/(name+'.compile.log')).write_text(compile_result.stdout)
    assert compile_result.returncode==0,(name,compile_result.stdout)
    result=subprocess.run([str(out/(name+'.test')),'--nocapture'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    (out/(name+'.run.log')).write_text(result.stdout)
    row={'name':name,'mutation_sha256':sha(body.encode()),'compile_exit':compile_result.returncode,'test_exit':result.returncode,'expected_failure':expected_failure,'command':cmd}
    if expected_failure:
        assert result.returncode!=0,(name,'mutant unexpectedly passed')
        assert 'test provider_context::'+expected_failure+' ... FAILED' in result.stdout,(name,result.stdout)
    else:
        assert result.returncode==0,(name,result.stdout)
    manifest['results'].append(row)
    (out/'attachment.rs').write_bytes(pristine)
    assert sha((out/'attachment.rs').read_bytes())==manifest['source_sha256']
    row['restored_sha256']=sha((out/'attachment.rs').read_bytes())
    print(name+': expected '+('named failure' if expected_failure else 'pass')+' confirmed',flush=True)
s=pristine.decode()
run('baseline',s)
mutations=[
 ('watermark_bound','.is_some_and(|watermark| watermark > terminal_prefix)','.is_some_and(|_| false)','admission_boundary_cannot_skip_pending_work_or_exceed_history'),
 ('reinstate_exact_publication_prefix',
  '        if self.recorded() == previous.recorded()\n',
  '''        if self.admission_watermark().is_some_and(|watermark| watermark != 0 && watermark != terminal_prefix) {
            return Err(ProviderContextAdmissionError::InvalidTransition);
        }
        if self.recorded() == previous.recorded()
''',
  'replacement_preserves_opening_boundary_after_terminal_prefix_grows'),
 ('same_id_boundary','&& self.admission_watermark() != previous.admission_watermark()','&& false','repeated_native_context_preserves_its_admission_boundary'),
 ('clear_admitted','return if previous.has_admitted_work(invocation_count) {','return if false {','missing_observations_do_not_authorize_clearing_an_admitted_context'),
 ('classify_admission','.is_some_and(|watermark| invocation_count != watermark)','.is_some_and(|_| false)','admission_watermark_distinguishes_old_history_from_new_context_work'),
]
for name,old,new,test in mutations:
    assert s.count(old)==1,(name,s.count(old))
    run(name,s.replace(old,new),test)
run('restored',s)
assert source.read_bytes()==pristine,'root source changed during probes'
assert tests.read_bytes()==testbytes,'root tests changed during probes'
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()==head
assert sha(rlib.read_bytes())==manifest['identity_rlib_sha256'],'identity artifact changed during probes'
manifest['root_source_unchanged']=True
manifest['root_tests_unchanged']=True
manifest['root_head_unchanged']=True
manifest['identity_rlib_unchanged']=True
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
lines=['# Isolated actual-source domain negative controls','',f"Source checkpoint: `{head}`.",f"Source SHA256: `{manifest['source_sha256']}`.",f"Test SHA256: `{manifest['tests_sha256']}`.",f"Identity dependency: `{rlib}`; SHA256 `{manifest['identity_rlib_sha256']}`.",f"Compiler: `{manifest['rustc_version']}`.",'','The harness compiles a hash-verified copy of the actual attachment.rs and the unchanged current provider_context.rs tests with rustc --test. Only ExecutionSessionId comes from an existing SDK rlib copied into this isolated evidence directory; the artifact supplies only the immutable identity type, while admission rules are compiled directly from the hash-verified current source copy. No replacement admission rules, Cargo invocation, shared artifact writes or root source edits are used.','','| Probe | Result | Expected failing regression |','| --- | --- | --- |']
for r in manifest['results']:
    log=(out/(r['name']+'.run.log')).read_text()
    summary=next(x for x in log.splitlines() if x.startswith('test result:'))
    lines.append(f"| {r['name']} | {summary} | {r['expected_failure'] or 'none'} |")
lines += ['','Each mutation starts from pristine source, records its exact unified diff and mutated hash, compiles successfully, and fails its named regression. The pristine copy is restored and hash-checked after each probe. The final restored suite passes 10/10. Root source, root tests, root HEAD and the identity rlib remained unchanged.','','The exact-publication-prefix mutant intentionally reintroduces the superseded constraint and must fail the C11 prefix-growth regression. The old a4 fresh-boundary removal mutant is not reused. These results qualify the copied actual domain rules in isolation. They do not qualify application namespace tracking, codecs, lifecycle orchestration, concurrency, integration, E2E, or the full crate/build selection. Compilation warnings and all run output are retained beside the manifest.']
(out/'README.md').write_text('\n'.join(lines)+'\n')
