use super::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    },
    thread,
};

struct CountRelease(Arc<AtomicUsize>);
impl Drop for CountRelease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn acquisition_is_shared_across_concurrent_host_operations() {
    let owner = Arc::new(DesktopHostOwnership::default());
    let acquisitions = Arc::new(AtomicUsize::new(0));
    let releases = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (owner, acquisitions, releases, barrier) = (
                owner.clone(),
                acquisitions.clone(),
                releases.clone(),
                barrier.clone(),
            );
            thread::spawn(move || {
                barrier.wait();
                let result = owner
                    .claim(
                        || {
                            acquisitions.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, ()>(DesktopHostClaim::Acquired(DesktopHostLease::new(
                                CountRelease(releases.clone()),
                            )))
                        },
                        || panic!("acquired claims do not wait"),
                    )
                    .unwrap();
                assert!(matches!(result, DesktopHostClaim::Acquired(_)));
            })
        })
        .collect();
    for task in threads {
        task.join().unwrap();
    }
    assert_eq!(acquisitions.load(Ordering::SeqCst), 1);
    assert_eq!(releases.load(Ordering::SeqCst), 0);
    drop(owner);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn busy_claim_has_no_resource_and_can_be_retried_after_owner_exit() {
    let owner = DesktopHostOwnership::default();
    let mut waits = 0;
    let result = owner
        .claim(
            || Ok::<_, ()>(DesktopHostClaim::Busy),
            || {
                waits += 1;
                waits < 3
            },
        )
        .unwrap();
    assert!(matches!(result, DesktopHostClaim::Busy));
    assert_eq!(waits, 3);
    let result = owner
        .claim(
            || Ok::<_, ()>(DesktopHostClaim::Acquired(DesktopHostLease::new(()))),
            || false,
        )
        .unwrap();
    assert!(matches!(result, DesktopHostClaim::Acquired(_)));
}

#[test]
fn bounded_restart_wait_can_acquire_after_predecessor_releases() {
    let owner = DesktopHostOwnership::default();
    let mut attempts = 0;
    let mut waits = 0;
    let result = owner
        .claim(
            || {
                attempts += 1;
                Ok::<_, ()>(if attempts == 3 {
                    DesktopHostClaim::Acquired(DesktopHostLease::new(()))
                } else {
                    DesktopHostClaim::Busy
                })
            },
            || {
                waits += 1;
                waits < 3
            },
        )
        .unwrap();
    assert!(matches!(result, DesktopHostClaim::Acquired(_)));
    assert_eq!((attempts, waits), (3, 2));
}

#[test]
fn failed_or_panicking_acquisition_does_not_publish_a_lease() {
    let owner = DesktopHostOwnership::default();
    assert!(owner
        .claim(|| Err::<DesktopHostClaim, _>("unreadable"), || false)
        .is_err());
    let panic = std::panic::catch_unwind(|| {
        owner.claim(
            || -> Result<DesktopHostClaim, ()> { panic!("physical claim failed") },
            || false,
        )
    });
    assert!(panic.is_err());
    let result = owner
        .claim(
            || Ok::<_, ()>(DesktopHostClaim::Acquired(DesktopHostLease::new(()))),
            || false,
        )
        .unwrap();
    assert!(matches!(result, DesktopHostClaim::Acquired(_)));
}

#[test]
fn admitted_worker_retains_lease_after_foreground_owner_disappears() {
    let owner = Arc::new(DesktopHostOwnership::default());
    let releases = Arc::new(AtomicUsize::new(0));
    drop(
        owner
            .claim(
                || {
                    Ok::<_, ()>(DesktopHostClaim::Acquired(DesktopHostLease::new(
                        CountRelease(releases.clone()),
                    )))
                },
                || false,
            )
            .unwrap(),
    );
    let worker_owner = owner.clone();
    let (entered, wait_entered) = std::sync::mpsc::channel();
    let (finish, wait_finish) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let _owner = worker_owner;
        entered.send(()).unwrap();
        wait_finish.recv().unwrap();
    });
    wait_entered.recv().unwrap();
    drop(owner);
    assert_eq!(releases.load(Ordering::SeqCst), 0);
    finish.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}
