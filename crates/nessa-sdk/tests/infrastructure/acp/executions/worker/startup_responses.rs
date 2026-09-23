//! Nested startup replies share the RPC deadline even when the real stdin pipe is full.
use super::*;
use std::{
    fs::File,
    io::{BufRead, BufReader, ErrorKind, Write},
    os::{fd::AsFd, unix::net::UnixListener},
    sync::{
        mpsc::{channel, Sender},
        Mutex,
    },
    thread::{self, JoinHandle},
};

const CHILD: &str = r#"
import array,fcntl,os,socket,sys,termios
os.write(1,sys.argv[2].encode())
s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect(sys.argv[1])
c=s.makefile('rwb',buffering=0)
remaining=int(c.readline())
while remaining:
    remaining-=len(os.read(0,remaining))
c.write(b'drained\n')
for line in c:
    size=array.array('i',[0]);fcntl.ioctl(0,termios.FIONREAD,size,True)
    c.write((str(size[0])+'\n').encode())
"#;
type PipeQuery = (String, oneshot::Sender<String>);
fn pipe_control(listener: UnixListener) -> (Sender<PipeQuery>, JoinHandle<()>) {
    let (requests, receiver) = channel::<PipeQuery>();
    let worker = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut socket = BufReader::new(socket);
        for (request, response) in receiver {
            socket.get_mut().write_all(request.as_bytes()).unwrap();
            let mut line = String::new();
            socket.read_line(&mut line).unwrap();
            let _ = response.send(line);
        }
    });
    (requests, worker)
}
async fn query(control: &Sender<PipeQuery>, request: String) -> String {
    let (response, result) = oneshot::channel();
    control.send((request, response)).unwrap();
    result.await.unwrap()
}
struct StartupAudit {
    reject: bool,
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for StartupAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            self.records.lock().unwrap().push(record);
            if self.reject {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn nested_startup_response_writes_observe_remaining_rpc_deadline() {
    for method in [
        "initialize",
        "session/new",
        "session/resume",
        "session/set_config_option",
    ] {
        for nested in ["unsupported/test", "session/request_permission"] {
            for reject_audit in [false, true] {
                let (root, mut config, capabilities) = profile_setup();
                config.max_frame_bytes = 16 * 1024 * 1024;
                config.max_incoming_frame_bytes = 16 * 1024 * 1024;
                config.shutdown_grace = Duration::from_millis(20);
                let socket = root.path().join("control.sock");
                let listener = UnixListener::bind(&socket).unwrap();
                // One short atomic stdout write puts both frames in the same read.
                let frames = format!(
                    "{}\n{}\n",
                    json!({"jsonrpc":"2.0","method":"ready"}),
                    json!({"jsonrpc":"2.0","id":77,"method":nested,"params":{}})
                );
                let mut process = tokio::process::Command::new("/usr/bin/python3");
                process.args(["-c", CHILD, socket.to_str().unwrap(), &frames]);
                let mut scope = ProcessScope::spawn(process).unwrap();
                let mut reader = Reader::new(
                    scope.stdout.take().unwrap(),
                    config.max_incoming_frame_bytes,
                );
                assert_eq!(
                    reader.next().await.unwrap().method.as_deref(),
                    Some("ready")
                );
                let (control, control_thread) = pipe_control(listener);
                // Measure actual pipe capacity, then have the child drain exactly
                // those bytes. It never reads stdin again. No clock or sleep guesses.
                let mut filler = File::from(
                    scope
                        .stdin
                        .as_ref()
                        .unwrap()
                        .as_fd()
                        .try_clone_to_owned()
                        .unwrap(),
                );
                let mut capacity = 0;
                loop {
                    match filler.write(&[b'x'; 4096]) {
                        Ok(size) => {
                            assert!(size > 0);
                            capacity += size;
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                        Err(error) => panic!("pipe fill failed: {error}"),
                    }
                }
                drop(filler);
                assert_eq!(query(&control, format!("{capacity}\n")).await, "drained\n");
                let empty = json_rpc::encode(
                    json_rpc::request(1, method, json!({"padding":""})),
                    config.max_frame_bytes,
                )
                .unwrap();
                let params = json!({"padding":"x".repeat(capacity-empty.len())});
                assert_eq!(
                    json_rpc::encode(
                        json_rpc::request(1, method, params.clone()),
                        config.max_frame_bytes
                    )
                    .unwrap()
                    .len(),
                    capacity
                );
                let (_commands, commands) = mpsc::channel(1);
                let (_close, close_requested) = watch::channel(None);
                let (operation_capabilities, _) =
                    watch::channel(ProviderOperationCapabilities::default());
                let (events, _events) = EventQueueBudget::new().channel(16);
                let audit = Arc::new(StartupAudit {
                    reject: reject_audit,
                    records: Mutex::new(Vec::new()),
                });
                let mut worker = Worker {
                    profile: TestAcpProfile {
                        reject_startup: false,
                        reject_session: false,
                    },
                    audit: audit.clone(),
                    cancellation_cause: None,
                    scope,
                    reader,
                    config,
                    capabilities,
                    commands,
                    close_requested,
                    events,
                    sequence: 0,
                    permission_sequence: Arc::new(AtomicU64::new(0)),
                    active: None,
                    steering: None,
                    steering_supported: false,
                    agent_accepts_images: false,
                    operation_capabilities,
                    permissions: HashMap::new(),
                    startup_advisory_session: None,
                    declined: None,
                    shutdown_deadline: None,
                    configured: true,
                    closing: false,
                    deferred_outcome: None,
                    provider_result: None,
                    settlement_facts: SettlementFacts::new(),
                    audit_sequence: 0,
                    operation_sequence: 0,
                    terminal_failure_source: None,
                    failure_cause: ObservationFailureCause::ExecutionFailed,
                };
                let mut execution = (method == "session/set_config_option").then(|| {
                    ExecutionController::new(ExecutionSessionId::new("restored-context").unwrap())
                });
                // Keep runnable work while querying the real OS under a paused
                // clock: only this test may advance the startup deadline.
                let clock_guard = tokio::spawn(async {
                    loop {
                        tokio::task::yield_now().await;
                    }
                });
                tokio::time::pause();
                let budget = Duration::from_millis(20);
                let deadline = Instant::now() + budget;
                let observed = {
                    let rpc = worker.rpc(method, params, deadline, execution.as_mut());
                    tokio::pin!(rpc);
                    loop {
                        let line = tokio::select! {biased;
                            result = &mut rpc => panic!("RPC settled before blocked response: {result:?}"),
                            line = query(&control, "count\n".into()) => line,
                        };
                        if line.trim().parse::<usize>().unwrap() == capacity {
                            break;
                        }
                    }
                    // Initial request consumed the entire pipe. The already-read
                    // nested request now requires a blocked response write.
                    poll_fn(|cx| {
                        assert!(rpc.as_mut().poll(cx).is_pending());
                        Poll::Ready(())
                    })
                    .await;
                    tokio::time::advance(budget + Duration::from_millis(1)).await;
                    poll_fn(|cx| Poll::Ready(rpc.as_mut().poll(cx))).await
                };
                tokio::time::resume();
                clock_guard.abort();
                let result = match observed.clone() {
                    Poll::Ready(result) => result.map(|_| ()),
                    Poll::Pending => Err(AgentError::Deadline),
                };
                let completed = worker.finish(&mut execution, result, &mut None).await;
                drop(control);
                control_thread.join().unwrap();
                assert_eq!(
                    observed,
                    Poll::Ready(Err(AgentError::Deadline)),
                    "{method}, {nested}, audit={reject_audit}"
                );
                assert_eq!(
                    worker.failure_cause,
                    ObservationFailureCause::DeadlineExceeded
                );
                assert_eq!(
                    worker.cancellation_cause,
                    Some((
                        PermissionCancellationReason::deadline_exceeded(),
                        CancellationOrigin::Runtime
                    ))
                );
                assert!(completed.cleanup.is_confirmed());
                assert_eq!(
                    completed.cleanup.operation_failure(),
                    Some(&AgentError::Deadline)
                );
                assert_eq!(
                    completed.failure,
                    Some(if reject_audit && method == "session/set_config_option" {
                        AgentError::OperationAndCleanupFailure {
                            operation_error: Box::new(AgentError::Deadline),
                            cleanup_error: Box::new(AgentError::AuditFailure),
                        }
                    } else {
                        AgentError::Deadline
                    })
                );
                assert_eq!(completed.settlement.provider_result(), None);
                let records = audit.records.lock().unwrap();
                if method == "session/set_config_option" {
                    assert_eq!(records.len(), 1);
                    let ExecutionAuditRecord::SessionClosed(record) = &records[0] else {
                        panic!("known context must retain closure");
                    };
                    assert_eq!(record.closure().session_id().as_str(), "restored-context");
                    assert_eq!(
                        record.closure().reason(),
                        &PermissionCancellationReason::deadline_exceeded()
                    );
                    assert_eq!(record.origin(), &CancellationOrigin::Runtime);
                    assert_eq!(
                        completed.cleanup.audit(),
                        &if reject_audit {
                            Err(AgentError::AuditFailure)
                        } else {
                            Ok(())
                        }
                    );
                } else {
                    assert!(records.is_empty());
                }
            }
        }
    }
}
