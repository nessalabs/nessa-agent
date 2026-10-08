//! Killable fault-isolation tests use Shepherd scopes and verify actual child execution.

pub(crate) fn run(test: &str, child_env: &str, timeout: std::time::Duration, marker: Option<&str>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let supervisor = shepherd::SupervisorBuilder::new().build();
            let spec = shepherd::ProcessSpec::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--nocapture"])
                .env(shepherd::EnvPolicy::Overrides(vec![(
                    child_env.into(),
                    Some("1".into()),
                )]))
                .output(shepherd::OutputMode::Capture {
                    buffer_bytes: 65536,
                    tail_bytes: 4096,
                });
            let options = shepherd::TerminateOptions {
                grace: shepherd::GracePeriod::new(std::time::Duration::from_millis(20)),
                force_timeout: Some(std::time::Duration::from_secs(2)),
            };
            let scoped = supervisor
                .with_scope_options(vec![], options, |scope| async move {
                    let pid = scope.spawn(spec).await.unwrap();
                    let output = scope.take_output(pid).unwrap();
                    let returned = tokio::time::timeout(timeout, scope.wait(pid)).await;
                    (pid, output, returned)
                })
                .await;
            let (pid, output, returned) = scoped.result.unwrap();
            let cleanup = scoped.termination.unwrap();
            let exit = supervisor.wait(pid).await.unwrap();
            let shutdown = supervisor.shutdown().await.unwrap();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let pipes = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    let snapshot = output.read();
                    assert_eq!(snapshot.dropped_bytes, 0);
                    assert!(snapshot.errors.is_empty(), "{:?}", snapshot.errors);
                    for chunk in snapshot.chunks {
                        assert!(stdout.len() + stderr.len() + chunk.bytes.len() <= 65536);
                        match chunk.stream {
                            shepherd::OutputStream::Stdout => stdout.extend(chunk.bytes),
                            shepherd::OutputStream::Stderr => stderr.extend(chunk.bytes),
                        }
                    }
                    if snapshot.stdout_closed && snapshot.stderr_closed {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await;
            let stdout = String::from_utf8(stdout).unwrap();
            let stderr = String::from_utf8(stderr).unwrap();
            eprintln!("child stdout: {stdout}");
            eprintln!("child stderr: {stderr}");
            eprintln!(
                "Shepherd cleanup verified: {}; exit: {exit:?}",
                cleanup.all_verified()
            );
            assert!(cleanup.all_verified());
            assert!(exit.outcome.is_verified());
            assert!(shutdown.scopes.iter().all(|scope| scope.all_verified()));
            assert!(pipes.is_ok(), "child output pipes did not close");
            assert!(
                returned.is_ok_and(|result| result.is_ok_and(|exit| exit.code == Some(0))),
                "isolated test must finish successfully before its deadline"
            );
            if let Some(marker) = marker {
                assert!(
                    stderr.contains(marker),
                    "child did not reach its required control marker"
                );
            }
            let summaries: Vec<_> = stdout
                .lines()
                .filter(|line| line.starts_with("test result:"))
                .collect();
            assert_eq!(summaries.len(), 1, "child must run exactly one test result");
            assert!(summaries[0].starts_with("test result: ok. 1 passed; 0 failed;"));
            assert!(stdout.contains(&format!("test {test} ... ok")));
        });
}
