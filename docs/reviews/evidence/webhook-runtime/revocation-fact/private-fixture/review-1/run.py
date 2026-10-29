import hashlib,json,os,shutil,subprocess
from pathlib import Path
src=Path('/workspace/nessa-webhook-revocation');out=Path('/tmp/nessa-private-fixture-independent-review')
shutil.copytree(src/'crates/nessa-auth',out/'auth',dirs_exist_ok=True)
env=dict(os.environ,PATH='/tmp/nessa-cargo/bin:'+os.environ['PATH'],CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
args=json.loads(Path('/tmp/nessa-revocation-independent-review/auth-args.json').read_text());args=[v.replace('/tmp/nessa-revocation-independent-review',str(out)) for v in args]
testargs=json.loads(Path('/tmp/nessa-revocation-independent-review/fact-args.json').read_text());testargs=[v.replace('/tmp/nessa-revocation-independent-review',str(out)) for v in testargs];testargs+=['-D','warnings','--extern','nessa_local_storage=/tmp/nessa-retention-dependency-snapshot/libnessa_local_storage-8949dced9d448800.rlib']
(out/'auth-args.json').write_text(json.dumps(args,indent=2)+'\n');(out/'fact-args.json').write_text(json.dumps(testargs,indent=2)+'\n')
def call(label,cmd):
 with(out/(label+'.txt')).open('w')as f:return subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT,env=env,preexec_fn=lambda:os.umask(0o022)).returncode
assert call('auth-compile',args)==0
assert call('fixed-compile',testargs)==0
assert call('fixed-tests',[str(out/'fact-tests'),'--nocapture'])==0
p=out/'auth/tests/revocation.rs';fixed=p.read_bytes();original=subprocess.check_output(['git','show','8c6e673b69055bdc1b2320e4f1b990b5c795aeba:crates/nessa-auth/tests/revocation.rs'],cwd=src)
assert original!=fixed
(out/'revert-edit.json').write_text(json.dumps({'originalSHA256':hashlib.sha256(original).hexdigest(),'fixedSHA256':hashlib.sha256(fixed).hexdigest(),'scope':'entire fixture-only before/after, exact Git base test file'},indent=2)+'\n')
try:
 p.write_bytes(original);assert p.read_bytes()==original
 assert call('reverted-compile',testargs)==0
 assert call('reverted-tests',[str(out/'fact-tests'),'--nocapture'])!=0
 text=(out/'reverted-tests.txt').read_text();assert '1 passed; 6 failed' in text and text.count('UnsafeFile')==6
finally:p.write_bytes(fixed);assert p.read_bytes()==fixed
assert call('restored-compile',testargs)==0
assert call('restored-tests',[str(out/'fact-tests'),'--nocapture'])==0
print('actual Auth compiled; 7 fixed pass; exact base fails 6 UnsafeFile; byte-restored 7 pass',flush=True)
