use crate::gateway::{
    application::MonotonicClock,
    domain::value_objects::{
        SystemdInvocationId, SystemdJobAttempt, SystemdJobMode, SystemdJobOperation,
        SystemdManagerIdentity, SystemdUnitName,
    },
};
use futures_lite::{future, StreamExt};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use zbus::{
    blocking::{connection::Builder as ConnectionBuilder, Connection, Proxy},
    zvariant::OwnedObjectPath,
    Proxy as AsyncProxy,
};

const SYSTEMD_DESTINATION: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
const DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UnitSnapshot {
    pub manager: SystemdManagerIdentity,
    pub object_path: String,
    pub id: String,
    pub names: Vec<String>,
    pub fragment_path: String,
    pub drop_in_paths: Vec<String>,
    pub active_state: String,
    pub sub_state: String,
    pub invocation: Option<SystemdInvocationId>,
    pub main_process_id: u32,
    pub service_type: String,
    pub restart: String,
    pub restart_microseconds: u64,
    pub timeout_stop_microseconds: u64,
    pub working_directory: String,
    pub environment: Vec<String>,
    pub exec_start_ex: Vec<ExecStartEx>,
    pub unit_file_state: String,
    pub unit_path: Vec<String>,
}

pub(super) type ExecStartEx = (
    String,
    Vec<String>,
    Vec<String>,
    u64,
    u64,
    u64,
    u64,
    u32,
    i32,
    i32,
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct JobTerminal {
    pub manager: SystemdManagerIdentity,
    pub object_path: String,
    pub job_id: u32,
    pub unit: SystemdUnitName,
    pub result: String,
}

pub(super) struct JobHandle {
    pub attempt: SystemdJobAttempt,
    terminal: mpsc::Receiver<Result<JobTerminal, String>>,
    cancellation: async_channel::Sender<()>,
}

pub(super) struct UserManager {
    connection: Connection,
    identity: SystemdManagerIdentity,
}

impl UserManager {
    pub fn connect(expected_uid: u32) -> Result<Self, String> {
        let connection = ConnectionBuilder::session()
            .map_err(|error| error.to_string())?
            .method_timeout(DBUS_METHOD_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let identity = manager_identity(&connection)?;
        if identity.user_id() != expected_uid {
            return Err("The systemd user-manager D-Bus owner has another UID".into());
        }
        {
            let proxy = manager_proxy(&connection, identity.unique_name())?;
            proxy.call::<_, _, ()>("Subscribe", &()).map_err(|error| {
                format!("The systemd user manager cannot be subscribed: {error}")
            })?;
        }
        Ok(Self {
            connection,
            identity,
        })
    }

    pub fn identity(&self) -> &SystemdManagerIdentity {
        &self.identity
    }

    pub fn recheck_identity(&self) -> Result<(), String> {
        let observed = manager_identity(&self.connection)?;
        (observed == self.identity)
            .then_some(())
            .ok_or_else(|| "The systemd user-manager owner changed during reconciliation".into())
    }

    pub fn unit_path(&self) -> Result<Vec<String>, String> {
        self.recheck_identity()?;
        manager_proxy(&self.connection, self.identity.unique_name())?
            .get_property("UnitPath")
            .map_err(|error| error.to_string())
    }

    pub fn environment(&self) -> Result<Vec<String>, String> {
        self.recheck_identity()?;
        manager_proxy(&self.connection, self.identity.unique_name())?
            .get_property("Environment")
            .map_err(|error| error.to_string())
    }

    pub fn reload(&self) -> Result<(), String> {
        self.recheck_identity()?;
        manager_proxy(&self.connection, self.identity.unique_name())?
            .call("Reload", &())
            .map_err(|error| error.to_string())
    }

    pub fn unit_file_state(&self, unit: &SystemdUnitName) -> Result<String, String> {
        self.recheck_identity()?;
        manager_proxy(&self.connection, self.identity.unique_name())?
            .call("GetUnitFileState", &(unit.as_str(),))
            .map_err(|error| error.to_string())
    }

    pub fn get_unit_by_pid(&self, process_id: u32) -> Result<String, String> {
        self.recheck_identity()?;
        let path: OwnedObjectPath = manager_proxy(&self.connection, self.identity.unique_name())?
            .call("GetUnitByPID", &(process_id,))
            .map_err(|error| error.to_string())?;
        Ok(path.to_string())
    }

    pub fn snapshot(&self, unit: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String> {
        self.recheck_identity()?;
        let manager = manager_proxy(&self.connection, self.identity.unique_name())?;
        let path: OwnedObjectPath = match manager.call("GetUnit", &(unit.as_str(),)) {
            Ok(path) => path,
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == NO_SUCH_UNIT => {
                return Ok(None);
            }
            Err(error) => return Err(error.to_string()),
        };
        let path_string = path.to_string();
        let unit_proxy = Proxy::new(
            &self.connection,
            self.identity.unique_name(),
            path.as_str(),
            "org.freedesktop.systemd1.Unit",
        )
        .map_err(|error| error.to_string())?;
        let service_proxy = Proxy::new(
            &self.connection,
            self.identity.unique_name(),
            path.as_str(),
            "org.freedesktop.systemd1.Service",
        )
        .map_err(|error| error.to_string())?;
        let invocation: Vec<u8> = unit_proxy
            .get_property("InvocationID")
            .map_err(|error| error.to_string())?;
        Ok(Some(UnitSnapshot {
            manager: self.identity.clone(),
            object_path: path_string,
            id: unit_proxy
                .get_property("Id")
                .map_err(|error| error.to_string())?,
            names: unit_proxy
                .get_property("Names")
                .map_err(|error| error.to_string())?,
            fragment_path: unit_proxy
                .get_property("FragmentPath")
                .map_err(|error| error.to_string())?,
            drop_in_paths: unit_proxy
                .get_property("DropInPaths")
                .map_err(|error| error.to_string())?,
            active_state: unit_proxy
                .get_property("ActiveState")
                .map_err(|error| error.to_string())?,
            sub_state: unit_proxy
                .get_property("SubState")
                .map_err(|error| error.to_string())?,
            invocation: if invocation.iter().all(|byte| *byte == 0) {
                None
            } else {
                Some(SystemdInvocationId::new(invocation).map_err(|error| error.to_string())?)
            },
            main_process_id: service_proxy
                .get_property("MainPID")
                .map_err(|error| error.to_string())?,
            service_type: service_proxy
                .get_property("Type")
                .map_err(|error| error.to_string())?,
            restart: service_proxy
                .get_property("Restart")
                .map_err(|error| error.to_string())?,
            restart_microseconds: service_proxy
                .get_property("RestartUSec")
                .map_err(|error| error.to_string())?,
            timeout_stop_microseconds: service_proxy
                .get_property("TimeoutStopUSec")
                .map_err(|error| error.to_string())?,
            working_directory: service_proxy
                .get_property("WorkingDirectory")
                .map_err(|error| error.to_string())?,
            environment: service_proxy
                .get_property("Environment")
                .map_err(|error| error.to_string())?,
            exec_start_ex: service_proxy
                .get_property("ExecStartEx")
                .map_err(|error| error.to_string())?,
            unit_file_state: manager
                .call("GetUnitFileState", &(unit.as_str(),))
                .map_err(|error| error.to_string())?,
            unit_path: manager
                .get_property("UnitPath")
                .map_err(|error| error.to_string())?,
        }))
    }

    pub fn enqueue(
        &self,
        operation: SystemdJobOperation,
        unit: &SystemdUnitName,
        clock: &dyn MonotonicClock,
    ) -> Result<JobHandle, String> {
        self.recheck_identity()?;
        let connection = self.connection.inner().clone();
        let identity = self.identity.clone();
        let unit = unit.clone();
        let (cancellation_sender, cancellation_receiver) = async_channel::bounded(1);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let (attempt_sender, attempt_receiver) = mpsc::sync_channel(1);
        let (terminal_sender, terminal_receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("nessa-systemd-job".into())
            .spawn(move || {
                let result: Result<(), String> = async_io::block_on(async {
                    let proxy = AsyncProxy::new(
                        &connection,
                        identity.unique_name(),
                        SYSTEMD_PATH,
                        SYSTEMD_MANAGER,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    let mut signals = proxy
                        .receive_signal("JobRemoved")
                        .await
                        .map_err(|error| error.to_string())?;
                    ready_sender
                        .send(Ok(()))
                        .map_err(|error| error.to_string())?;
                    let method = match operation {
                        SystemdJobOperation::Start => "StartUnit",
                        SystemdJobOperation::Stop => "StopUnit",
                    };
                    let path: OwnedObjectPath = proxy
                        .call(method, &(unit.as_str(), SystemdJobMode::Fail.as_str()))
                        .await
                        .map_err(|error| error.to_string())?;
                    let path_string = path.to_string();
                    let job_id = path_string
                        .strip_prefix("/org/freedesktop/systemd1/job/")
                        .and_then(|value| value.parse().ok())
                        .ok_or_else(|| {
                            "The systemd job path has no canonical numeric ID".to_string()
                        })?;
                    let attempt = SystemdJobAttempt::new(
                        identity.clone(),
                        operation,
                        SystemdJobMode::Fail,
                        unit.clone(),
                        path_string,
                        job_id,
                    )
                    .map_err(|error| error.to_string())?;
                    attempt_sender
                        .send(Ok(attempt.clone()))
                        .map_err(|error| error.to_string())?;
                    loop {
                        enum SignalPoll {
                            Message(Option<zbus::Message>),
                            Cancelled,
                        }
                        let message = match future::race(
                            async { SignalPoll::Message(signals.next().await) },
                            async {
                                let _ = cancellation_receiver.recv().await;
                                SignalPoll::Cancelled
                            },
                        )
                        .await
                        {
                            SignalPoll::Message(Some(message)) => message,
                            SignalPoll::Message(None) => {
                                return Err("The systemd job signal stream disconnected".into());
                            }
                            SignalPoll::Cancelled => return Ok(()),
                        };
                        let (signal_id, signal_path, signal_unit, result): (
                            u32,
                            OwnedObjectPath,
                            String,
                            String,
                        ) = message
                            .body()
                            .deserialize()
                            .map_err(|error| error.to_string())?;
                        let parsed_unit = SystemdUnitName::parse(signal_unit)
                            .map_err(|error| error.to_string())?;
                        if attempt.agrees_with_terminal(
                            &identity,
                            signal_path.as_str(),
                            signal_id,
                            &parsed_unit,
                        ) {
                            terminal_sender
                                .send(Ok(JobTerminal {
                                    manager: identity.clone(),
                                    object_path: signal_path.to_string(),
                                    job_id: signal_id,
                                    unit: parsed_unit,
                                    result,
                                }))
                                .map_err(|error| error.to_string())?;
                            return Ok(());
                        }
                    }
                });
                if let Err(error) = result {
                    let _ = ready_sender.send(Err(error.clone()));
                    let _ = attempt_sender.send(Err(error.clone()));
                    let _ = terminal_sender.send(Err(error));
                }
            })
            .map_err(|error| format!("The systemd job worker could not start: {error}"))?;
        let ready = receive_until(
            &ready_receiver,
            clock,
            clock.now() + Duration::from_secs(5),
            "Timed out installing the systemd JobRemoved subscription",
        );
        match ready {
            Ok(Ok(())) => {}
            Ok(Err(error)) | Err(error) => {
                let _ = cancellation_sender.try_send(());
                return Err(error);
            }
        }
        let attempt = receive_until(
            &attempt_receiver,
            clock,
            clock.now() + Duration::from_secs(30),
            "The systemd enqueue reply was lost or timed out",
        );
        let attempt = match attempt {
            Ok(Ok(attempt)) => attempt,
            Ok(Err(error)) | Err(error) => {
                let _ = cancellation_sender.try_send(());
                return Err(error);
            }
        };
        Ok(JobHandle {
            attempt,
            terminal: terminal_receiver,
            cancellation: cancellation_sender,
        })
    }
}

fn receive_until<T>(
    receiver: &mpsc::Receiver<T>,
    clock: &dyn MonotonicClock,
    deadline: Instant,
    timeout: &str,
) -> Result<T, String> {
    loop {
        match receiver.try_recv() {
            Ok(value) => return Ok(value),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("The systemd worker disconnected".into())
            }
            Err(mpsc::TryRecvError::Empty) if clock.now() < deadline => {
                clock.wait(Duration::from_millis(10));
            }
            Err(mpsc::TryRecvError::Empty) => return Err(timeout.into()),
        }
    }
}

impl JobHandle {
    pub fn wait(
        self,
        clock: &dyn MonotonicClock,
        deadline: Instant,
    ) -> Result<JobTerminal, String> {
        loop {
            match self.terminal.try_recv() {
                Ok(result) => return result,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("The systemd job result stream disconnected".into())
                }
                Err(mpsc::TryRecvError::Empty) if clock.now() < deadline => {
                    clock.wait(Duration::from_millis(10));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    return Err(
                        "The systemd job result was not observed before its deadline".into(),
                    )
                }
            }
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        let _ = self.cancellation.try_send(());
    }
}

pub(super) fn verify_linger(expected_uid: u32) -> Result<(), String> {
    let connection = ConnectionBuilder::system()
        .map_err(|error| error.to_string())?
        .method_timeout(DBUS_METHOD_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let manager = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .map_err(|error| error.to_string())?;
    let path: OwnedObjectPath = manager
        .call("GetUser", &(expected_uid,))
        .map_err(|error| error.to_string())?;
    let user = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        path.as_str(),
        "org.freedesktop.login1.User",
    )
    .map_err(|error| error.to_string())?;
    let uid: u32 = user
        .get_property("UID")
        .map_err(|error| error.to_string())?;
    let linger: bool = user
        .get_property("Linger")
        .map_err(|error| error.to_string())?;
    if uid != expected_uid {
        return Err("The logind user record has another UID".into());
    }
    linger.then_some(()).ok_or_else(|| {
        "Linger is disabled for this account; Nessa made no service or linger change".into()
    })
}

fn manager_identity(connection: &Connection) -> Result<SystemdManagerIdentity, String> {
    let bus = Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .map_err(|error| error.to_string())?;
    let owner: String = bus
        .call("GetNameOwner", &(SYSTEMD_DESTINATION,))
        .map_err(|error| error.to_string())?;
    let process_id: u32 = bus
        .call("GetConnectionUnixProcessID", &(owner.as_str(),))
        .map_err(|error| error.to_string())?;
    let user_id: u32 = bus
        .call("GetConnectionUnixUser", &(owner.as_str(),))
        .map_err(|error| error.to_string())?;
    SystemdManagerIdentity::new(owner, process_id, user_id).map_err(|error| error.to_string())
}

fn manager_proxy<'a>(
    connection: &'a Connection,
    destination: &'a str,
) -> Result<Proxy<'a>, String> {
    Proxy::new(connection, destination, SYSTEMD_PATH, SYSTEMD_MANAGER)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct AdvancingClock(Mutex<Instant>);

    impl MonotonicClock for AdvancingClock {
        fn now(&self) -> Instant {
            *self.0.lock().unwrap()
        }

        fn wait(&self, duration: Duration) {
            *self.0.lock().unwrap() += duration;
        }
    }

    #[test]
    fn worker_waits_use_the_injected_deadline() {
        let clock = AdvancingClock(Mutex::new(Instant::now()));
        let (sender, receiver) = mpsc::channel();
        sender.send(7).unwrap();
        assert_eq!(
            receive_until(
                &receiver,
                &clock,
                clock.now() + Duration::from_secs(1),
                "late"
            ),
            Ok(7)
        );

        let (_sender, receiver) = mpsc::channel::<u8>();
        let deadline = clock.now() + Duration::from_millis(20);
        assert_eq!(
            receive_until(&receiver, &clock, deadline, "late"),
            Err("late".into())
        );
        assert!(clock.now() >= deadline);
    }

    #[test]
    fn dropping_a_job_handle_cancels_its_signal_wait() {
        let manager = SystemdManagerIdentity::new(":1.7".into(), 41, 1000).unwrap();
        let unit = SystemdUnitName::parse("nessa-gateway.service".into()).unwrap();
        let attempt = SystemdJobAttempt::new(
            manager,
            SystemdJobOperation::Start,
            SystemdJobMode::Fail,
            unit,
            "/org/freedesktop/systemd1/job/9".into(),
            9,
        )
        .unwrap();
        let (_terminal_sender, terminal) = mpsc::channel();
        let (cancellation, cancelled) = async_channel::bounded(1);
        drop(JobHandle {
            attempt,
            terminal,
            cancellation,
        });
        assert_eq!(cancelled.try_recv(), Ok(()));
    }
}
