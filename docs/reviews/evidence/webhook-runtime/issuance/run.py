from pathlib import Path
import os,subprocess,json
root=Path('/workspace/nessa-webhook-runtime')
env=os.environ|dict(PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup',CARGO_TARGET_DIR='/workspace/nessa-agent/target',CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0')
policy=root/'crates/nessa-auth/src/adapters/cedar/policies.cedar'
registry=root/'crates/nessa-auth/src/adapters/local/registry.rs'
attributes=root/'crates/nessa-auth/src/adapters/cedar/attributes.rs'
cases=[('hold_kind',policy,'(principal.kind != "integration" || context.action != "conversation.read")','context.action != "conversation.read"','webhook_path_holds_only'),('hold_action',policy,'(principal.kind != "integration" || context.action != "conversation.read")','principal.kind != "integration"','webhook_issue_asks_cedar'),('issue_kind',policy,'when { context.path == "webhook" && principal.kind != "integration" };','when { false };','who_may_be_issued'),('path_mapping',attributes,'IssuePath::WebhookIssue => "webhook"','IssuePath::WebhookIssue => "product"','webhook_issue_asks_cedar'),('path_use',registry,'CredentialIssuePurpose::Webhook => IssuePath::WebhookIssue','CredentialIssuePurpose::Webhook => IssuePath::ProductIssue','webhook_issue_asks_cedar'),('receipt_purpose',registry,'if request.purpose == CredentialIssuePurpose::Webhook {','if false {','issuing_purpose_is_bound')]
results=[]
for name,path,before,after,test in cases:
 original=path.read_bytes(); assert original.count(before.encode())==1,(name,'replacement')
 try:
  path.write_bytes(original.replace(before.encode(),after.encode()))
  run=subprocess.run(['cargo','test','-p','nessa-auth','--lib',test],cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
  Path('/tmp/nessa-runtime-issue-probes/'+name+'-mutated.txt').write_bytes(run.stdout)
  assert run.returncode!=0 and b'FAILED' in run.stdout,(name,run.stdout[-1500:])
 finally:path.write_bytes(original)
 run=subprocess.run(['cargo','test','-p','nessa-auth','--lib',test],cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 Path('/tmp/nessa-runtime-issue-probes/'+name+'-restored.txt').write_bytes(run.stdout)
 assert run.returncode==0,(name,run.stdout[-1500:])
 results.append(dict(probe=name,mutation='caught',restored='passed',byte_identical=path.read_bytes()==original));print(name,flush=True)
Path('/tmp/nessa-runtime-issue-probes/results.json').write_text(json.dumps(results,indent=2)+'\n')
