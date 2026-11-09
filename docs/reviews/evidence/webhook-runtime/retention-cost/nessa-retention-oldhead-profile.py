from pathlib import Path
import json,subprocess,os
h=Path('/tmp/nessa-retention-oldhead-probes');root=Path('/workspace/nessa-webhook-composition');env=os.environ.copy();env.update(CARGO_HOME='/tmp/nessa-cargo',RUSTUP_HOME='/tmp/nessa-rustup')
a=json.loads(Path('/tmp/nessa-origin-independent-review/auth-args.json').read_text());a=[v.replace(str(root),str(h)).replace('/tmp/nessa-origin-independent-review',str(h)) for v in a];(h/'auth-args.json').write_text(json.dumps(a,indent=2))
with (h/'auth-compile.txt').open('w') as out:rc=subprocess.run(a,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
assert rc==0,'old auth compile'
driver=Path('/tmp/nessa-origin-probes/lib.rs').read_text().replace('/tmp/nessa-origin-probes',str(h));(h/'lib.rs').write_text(driver)
p=h/'crates/nessa-server/tests/webhooks/infrastructure/outbox.rs';original=p.read_text();(h/'outbox-original.rs').write_text(original)
helpers='''\nstatic PROFILE_VM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
unsafe extern "C" fn profile_vm(_: *mut std::ffi::c_void) -> std::ffi::c_int {
    PROFILE_VM.fetch_add(100, std::sync::atomic::Ordering::Relaxed); 0
}
fn start_profile(store: &SqlOutbox) -> std::time::Instant {
    let connection = store.owner.connection.lock().unwrap();
    unsafe { nessa_local_database::rusqlite::ffi::sqlite3_progress_handler(connection.handle(), 100, Some(profile_vm), std::ptr::null_mut()); }
    std::time::Instant::now()
}
fn profile_position(label: &str, position: u64, begin: std::time::Instant) {
    if [1,128,256,512,768,1024,1025,1536,2048].contains(&position) {
        eprintln!("PROFILE {} position={} elapsed_ms={} vm_steps_approx={}",label,position,begin.elapsed().as_millis(),PROFILE_VM.load(std::sync::atomic::Ordering::Relaxed));
    }
}
'''
s=original+helpers
for name,label in [('terminal_retention_is_bounded','terminal'),('stop_retention_is_bounded_when_publication_fails','stop')]:
 start=s.index('async fn '+name+'()');end=s.index('\n#[tokio::test]',start+1)
 section=s[start:end].replace('let f = Fixture::new().await;','let f = Fixture::new().await;\n    let profile_begin = start_profile(&f.store);',1)
 if label=='terminal':section=section.replace('assert_eq!(sent.phase(), Phase::Terminal(Outcome::Sent));','assert_eq!(sent.phase(), Phase::Terminal(Outcome::Sent));\n        profile_position("terminal",position,profile_begin);')
 else:
  needle='''            .unwrap();
    }
    for position in RETAINED_LIMIT'''
  assert needle in section
  section=section.replace(needle,'''            .unwrap();
        profile_position("stop-terminal-setup",position,profile_begin);
    }
    for position in RETAINED_LIMIT''',1)
  section=section.replace('''        f.store.queue(f.record(position)).await.unwrap();
    }
    let held''','''        f.store.queue(f.record(position)).await.unwrap();
        profile_position("stop-pending-setup",position,profile_begin);
    }
    eprintln!("PROFILE stop setup_done_ms={} vm_steps_approx={}",profile_begin.elapsed().as_millis(),PROFILE_VM.load(std::sync::atomic::Ordering::Relaxed));
    let held''',1)
 s=s[:start]+section+s[end:]
p.write_text(s)
args=json.loads(Path('/tmp/nessa-origin-probes/args.json').read_text());args=[v.replace('/tmp/nessa-origin-probes',str(h)) for v in args];args=[v if not v.startswith('nessa_auth=') else f'nessa_auth={h}/libnessa_auth.rlib' for v in args];(h/'args.json').write_text(json.dumps(args,indent=2))
with (h/'compile.txt').open('w') as out: rc=subprocess.run(args,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
assert rc==0,'old server probe compile'
for label,name in [('terminal','terminal_retention_is_bounded'),('stop','stop_retention_is_bounded_when_publication_fails')]:
 command=['strace','-f','-c','-e','trace=fsync,fdatasync,openat,rename,renameat,link,linkat,unlink,fcntl,pwrite64,write','-o',str(h/(label+'-syscalls.txt')),str(h/'tests-bin'),'--exact','webhooks::infrastructure::outbox::store::tests::'+name,'--nocapture']
 (h/(label+'-command.json')).write_text(json.dumps(command,indent=2))
 with (h/(label+'-profile.txt')).open('w') as out: rc=subprocess.run(command,env=env,stdout=out,stderr=subprocess.STDOUT).returncode
 assert rc==0,(label,rc)
 print(label,'passed',flush=True)
