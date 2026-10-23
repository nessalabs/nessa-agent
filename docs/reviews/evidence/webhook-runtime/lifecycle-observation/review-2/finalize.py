import pathlib,json,subprocess,os,hashlib
r=pathlib.Path('/tmp/nessa-expiry-context-final-review');p=r/'probes.rs';original=p.read_bytes();args=json.loads((r/'args.json').read_text());env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
old='assert!(restoration::validate(&c).is_ok(),"diagnostic: expected current implementation acceptance");';new='assert_eq!(restoration::validate(&c),Err(OutboxError::Invalid),"owed issuance prerequisite contradicts retained ExpiryObserved");';assert original.decode().count(old)==1
try:
 p.write_text(original.decode().replace(old,new));(r/'issue-required-rejection-probes.rs').write_bytes(p.read_bytes())
 with (r/'issue-required-rejection-compile.txt').open('w')as f:rc=subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT).returncode
 assert rc==0
 with (r/'issue-required-rejection-fails.txt').open('w')as f:rc=subprocess.run([str(r/'tests-bin'),'probe_reintroduced_owed_issue_after_expiry_observation','--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT).returncode
 assert rc!=0
finally:p.write_bytes(original)
with (r/'final-compile.txt').open('w')as f:rc=subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT).returncode
assert rc==0
with (r/'final-tests.txt').open('w')as f:rc=subprocess.run([str(r/'tests-bin'),'--test-threads=2'],env=env,stdout=f,stderr=subprocess.STDOUT).returncode
assert rc==0
s=pathlib.Path('/workspace/nessa-webhook-composition'); hashes={}
for rel in ['crates/nessa-auth','crates/nessa-server/src/webhooks']:
 for path in sorted((s/rel).rglob('*')):
  if path.is_file():
   relative=path.relative_to(s);a=path.read_bytes();b=(r/relative).read_bytes();assert a==b,str(relative);hashes[str(relative)]=hashlib.sha256(a).hexdigest()
(r/'production-source-provenance.json').write_text(json.dumps({'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=s,text=True).strip(),'status':subprocess.check_output(['git','status','--short'],cwd=s,text=True),'base':'6a107073f9261dcfcaedb37b9c8c9fe773c1d470','productionCopiesByteIdentical':True,'hashes':hashes},indent=2))
print('final tests pass; all production copies byte-identical; checkout status clean')
