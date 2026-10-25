from pathlib import Path
import subprocess, os, hashlib, json
source=Path('/workspace/nessa-test-waits/scripts/rust-test-support/isolated.rs').read_text()
path=Path('/tmp/nessa-isolation-probes/isolated.rs')
env=os.environ | {'CARGO_HOME':'/tmp/nessa-cargo','RUSTUP_HOME':'/tmp/nessa-rustup'}
probes=[
('terminal-readiness','ready: directory.path().join("ready").exists(),','ready: entered.is_some(),','terminal_marker_proves_entry_even_when_no_running_sample_saw_it'),
('old-five-second-startup','None if began.elapsed() >= startup','None if began.elapsed() >= Duration::from_secs(5)','startup_does_not_spend_the_execution_budget'),
('lost-diagnostics','let (stderr, err_truncated) = tail(&stderr);','let (_, err_truncated) = tail(&stderr);\n    let stderr = String::new();','failed_child_retains_its_diagnostic'),
('phase-correlation','Some(Phase::Execution)','Some(Phase::Startup)','hung_children_are_killed_waited_and_identified_by_phase'),
]
results=[]
for name,old,new,test in probes:
    assert old in source,name
    path.write_text(source.replace(old,new,1))
    r=subprocess.run(['/tmp/nessa-cargo/bin/cargo','test','--manifest-path','/tmp/nessa-isolation-probes/Cargo.toml','--lib',test,'--','--nocapture'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
    Path('/tmp/nessa-isolation-probes/'+name+'.log').write_bytes(r.stdout)
    results.append({'probe':name,'exit':r.returncode,'expected':'test assertion failure','sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
    print(name,r.returncode,flush=True)
    assert r.returncode==101,(name,r.stdout[-3000:])
path.write_text(source)
r=subprocess.run(['/tmp/nessa-cargo/bin/cargo','test','--manifest-path','/tmp/nessa-isolation-probes/Cargo.toml','--lib'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
Path('/tmp/nessa-isolation-probes/restored.log').write_bytes(r.stdout)
results.append({'restored':r.returncode,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
Path('/tmp/nessa-isolation-probes/results.json').write_text(json.dumps(results,indent=2)+'\n')
print('restored',r.returncode,flush=True)
assert r.returncode==0
