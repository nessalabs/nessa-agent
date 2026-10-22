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
            let exit = supervisor.wait(pid).await;
            let shutdown = supervisor.shutdown().await;
            let (captured, pipes) = capture_output(|| output.read()).await;
            eprintln!(
                "child stdout: {}",
                String::from_utf8_lossy(&captured.stdout)
            );
            eprintln!(
                "child stderr: {}",
                String::from_utf8_lossy(&captured.stderr)
            );
            eprintln!("child wait result: {returned:?}; exit: {exit:?}");
            eprintln!(
                "Shepherd cleanup verified: {}; cleanup: {cleanup:?}; shutdown: {shutdown:?}",
                cleanup.all_verified()
            );
            eprintln!(
                "output capture: pipes={pipes:?}; closed={:?}; dropped_bytes={}; errors={:?}",
                captured.closed, captured.dropped_bytes, captured.errors
            );
            captured.assert_clean();
            let stdout = String::from_utf8(captured.stdout).unwrap();
            let stderr = String::from_utf8(captured.stderr).unwrap();
            let exit = exit.unwrap();
            let shutdown = shutdown.unwrap();
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

#[derive(Default)]
struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    dropped_bytes: u64,
    errors: Vec<String>,
    closed: (bool, bool),
}

impl CapturedOutput {
    fn assert_clean(&self) {
        assert_eq!(self.dropped_bytes, 0);
        assert!(self.errors.is_empty(), "{:?}", self.errors);
    }
}

async fn capture_output(
    mut read: impl FnMut() -> shepherd::OutputSnapshot,
) -> (CapturedOutput, Result<(), tokio::time::error::Elapsed>) {
    let mut captured = CapturedOutput::default();
    let pipes = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let snapshot = read();
            captured.dropped_bytes = captured.dropped_bytes.max(snapshot.dropped_bytes);
            // Shepherd retains cumulative errors for its two streams. Replace
            // the last nonempty observation rather than duplicating it per read.
            if !snapshot.errors.is_empty() {
                captured.errors = snapshot.errors;
            }
            captured.closed = (snapshot.stdout_closed, snapshot.stderr_closed);
            for chunk in snapshot.chunks {
                assert!(captured.stdout.len() + captured.stderr.len() + chunk.bytes.len() <= 65536);
                match chunk.stream {
                    shepherd::OutputStream::Stdout => captured.stdout.extend(chunk.bytes),
                    shepherd::OutputStream::Stderr => captured.stderr.extend(chunk.bytes),
                }
            }
            if snapshot.stdout_closed && snapshot.stderr_closed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    (captured, pipes)
}

#[cfg(test)]
mod diagnostics_tests {
    use super::*;

    #[test]
    fn incomplete_capture_retains_child_summary_before_rejecting_capture_facts() {
        for (dropped_bytes, errors) in [
            (0, vec!["synthetic capture failure".to_owned()]),
            (1, Vec::new()),
        ] {
            let output = shepherd::OutputSnapshot {
                chunks: vec![shepherd::OutputChunk {
                    stream: shepherd::OutputStream::Stdout,
                    bytes: b"test child ... ok\ntest result: ok. 1 passed; 0 failed;\n".to_vec(),
                }],
                tail: Vec::new(),
                dropped_bytes,
                stdout_closed: true,
                stderr_closed: true,
                errors: errors.clone(),
            };
            let (captured, pipes) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(capture_output(|| output.clone()));
            assert!(pipes.is_ok());
            assert_eq!(captured.closed, (true, true));
            assert_eq!(captured.dropped_bytes, dropped_bytes);
            assert_eq!(captured.errors, errors);
            assert!(String::from_utf8_lossy(&captured.stdout)
                .contains("test result: ok. 1 passed; 0 failed;"));
            assert!(
                std::panic::catch_unwind(|| captured.assert_clean()).is_err(),
                "captured diagnostics must never turn an incomplete capture into success"
            );
        }
    }
}
