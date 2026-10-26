//! The tunnel's program across the gateway's runs: what a bind failure and
//! a status from shepherd become.
use super::*;
use shepherd::KeptSpec;
use std::time::Duration;

/// Ends a kept program's group when dropped, so a failed assertion leaves
/// nothing running. Disarmed once the test has seen it end, so a group id
/// another test's program has since taken is never signalled.
struct EndsGroup(Option<u32>);
impl EndsGroup {
    fn ended(mut self) {
        self.0 = None;
    }
}
impl Drop for EndsGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // SAFETY: kill takes plain integers; the group is the test's
            // own kept program, still running.
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
    }
}

fn alive(pid: u32) -> bool {
    // Signal 0 checks for the process without signalling it.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// A bind failure ends the program an earlier run left running, which
/// would otherwise go on forwarding to whatever holds the port, and leaves
/// Settings comparing with no running tunnel.
#[tokio::test]
async fn an_ingress_that_cannot_be_bound_ends_the_kept_program() {
    let namespace = tempfile::tempdir().unwrap();
    let files = TunnelFiles::new(namespace.path());
    std::fs::create_dir_all(files.directory()).unwrap();
    let spec = KeptSpec::new(
        service_name(),
        shepherd::ProcessSpec::new("/bin/sleep").arg("60"),
    )
    .log(files.directory().join("cloudflared.log"));
    let process = kept(&files).unwrap().start(spec).await.unwrap();
    let pid = process.os_pid();
    let ends = EndsGroup(Some(pid));
    // Left running, as a gateway stop leaves it.
    process.release();
    assert!(alive(pid));
    std::fs::write(files.directory().join(UNKEPT_RECORD), b"{}").unwrap();

    let prepared = prepare(
        namespace.path(),
        &TunnelConfig {
            ingress: "127.0.0.1:47651".parse().unwrap(),
            mode: TunnelMode::Quick,
            hostname: None,
        },
    );
    let status = prepared.view().status;
    let settings = prepared.running();
    assert!(settings.borrow().is_some());
    prepared.ingress_failed().await.unwrap();

    tokio::time::timeout(Duration::from_secs(15), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the kept program is ended");
    ends.ended();
    assert!(kept(&files).unwrap().list().await.unwrap().is_empty());
    assert!(!files.directory().join(UNKEPT_RECORD).exists());
    assert!(
        settings.borrow().is_none(),
        "nothing runs for Settings to hold"
    );
    assert_eq!(
        *status.borrow(),
        TunnelStatus::Failed(TunnelFailure::Unavailable("ingress"))
    );
}

#[test]
fn shepherd_statuses_read_as_the_tunnels() {
    let address =
        nessa_protocol::tunnel::TunnelAddress::try_from("https://a.trycloudflare.com".to_owned())
            .unwrap();
    let backoff = |reason: &str| {
        tunnel_status(&ServiceStatus::Backoff {
            failures: 1,
            delay: Duration::from_secs(1),
            reason: reason.into(),
        })
    };
    assert_eq!(
        tunnel_status(&ServiceStatus::Starting { attempt: 1 }),
        Some(TunnelStatus::Starting)
    );
    // A start after a failure keeps the failure's reason showing.
    assert_eq!(tunnel_status(&ServiceStatus::Starting { attempt: 2 }), None);
    assert_eq!(
        tunnel_status(&ServiceStatus::Ready {
            value: address.clone(),
            os_pid: 7,
            adopted: true,
        }),
        Some(TunnelStatus::Ready(address))
    );
    for word in TunnelFailure::UNAVAILABLE {
        assert_eq!(
            backoff(&format!("launch: {word}")),
            Some(TunnelStatus::Failed(TunnelFailure::Unavailable(word)))
        );
    }
    assert_eq!(
        backoff("not ready in time"),
        Some(TunnelStatus::Failed(TunnelFailure::NotReady))
    );
    for other in [
        "exited before ready: exited with code 1",
        "exited with code 0",
        "spawn: no such file",
        "kept: kept record store failed: x",
        "launch: something new",
    ] {
        assert_eq!(
            backoff(other),
            Some(TunnelStatus::Failed(TunnelFailure::Start)),
            "{other}"
        );
    }
    assert_eq!(
        tunnel_status(&ServiceStatus::Failed {
            reason: "launch: download".into()
        }),
        Some(TunnelStatus::Failed(TunnelFailure::Unavailable("download")))
    );
    assert_eq!(tunnel_status(&ServiceStatus::Stopping), None);
    assert_eq!(tunnel_status(&ServiceStatus::Released { os_pid: 7 }), None);
}

/// With the tunnel off and nothing ever kept, nothing is created.
#[tokio::test]
async fn releasing_with_nothing_kept_creates_nothing() {
    let namespace = tempfile::tempdir().unwrap();
    release_kept(namespace.path()).await.unwrap();
    assert!(!TunnelFiles::new(namespace.path()).directory().exists());
}
