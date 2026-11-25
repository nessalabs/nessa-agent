//! Cloud accounts: the saved connection to each cloud a Cloud Box can be
//! made on, which cloud new boxes go to, and the provider port that makes,
//! wakes, stops and removes a VM there (ADR 1417). Every gateway runs only
//! its own agents, so no conversation runs in a VM through this gateway; a
//! Cloud Box runs a gateway of its own.
pub use crate::domain::RestCause;
use crate::workspaces::domain::{
    AdmissionRejection, Admitted, BoatConnection, Cloud, CloudAccount, CloudConnection, Failure,
    RefusalDisposition, VmSpec,
};
use async_trait::async_trait;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// A VM's runtime connection, as a Cloud Box's bootstrap uses it.
#[async_trait]
pub trait WorkspaceRuntime: Send + Sync {
    /// Run `command` in the VM with `input` as its standard input, and
    /// answer the end of what it printed.
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String>;
}
#[async_trait]
pub trait Settings: Send + Sync {
    async fn read(&self) -> Result<CloudConnection, Failure>;
    fn generation(&self) -> Result<u64, Failure>;
    async fn save(&self, generation: u64, connection: CloudConnection) -> Result<(), Failure>;
    async fn clear(&self) -> Result<(), Failure>;
}
/// The provider retains an actual target even when post-create admission fails.
pub struct WorkspaceReceipt {
    pub vm: String,
    pub admission: Admitted,
    pub refusal: Option<AdmissionRejection>,
    pub disposition: RefusalDisposition,
}
#[async_trait]
pub trait Provider: Send + Sync {
    /// Check a connection about to be saved: its account, and that a VM
    /// made from `spec` could be.
    async fn validate(&self, account: &CloudAccount, spec: &VmSpec) -> Result<(), Failure>;
    /// Make a VM from `spec` under the provider key `key`: asked again with
    /// the same key, the provider finds the one it made.
    async fn create(
        &self,
        account: &CloudAccount,
        spec: &VmSpec,
        key: &str,
    ) -> Result<WorkspaceReceipt, Failure>;
    /// Reach `vm` (waking it), made under `key` and admitted as `admission`.
    async fn connect(
        &self,
        account: &CloudAccount,
        spec: &VmSpec,
        key: &str,
        vm: &str,
        admission: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure>;
    /// Best effort rollback of a received creation receipt that could not be saved.
    /// Acceptance alone cannot be returned as confirmed cleanup.
    /// `admission` is what the VM was admitted under when it was made.
    async fn remove(
        &self,
        account: &CloudAccount,
        vm: &str,
        admission: &Admitted,
    ) -> Result<(), Failure>;
    /// Stop `vm`, keeping its disk: it costs nothing while stopped, and
    /// [`Provider::connect`] resumes it.
    async fn stop(&self, account: &CloudAccount, vm: &str, cause: RestCause)
        -> Result<(), Failure>;
}

/// Why the cloud for new boxes changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceCause {
    /// Someone chose it.
    Chosen,
    /// The cloud new boxes were going to was disconnected.
    Disconnected,
}
impl ChoiceCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chosen => "chosen",
            Self::Disconnected => "disconnected",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryEvent {
    /// New boxes are about to go to `after` instead of `before`, for `cause`,
    /// at `by`'s request. Recorded before the choice is saved.
    CloudChosen {
        before: Cloud,
        after: Cloud,
        cause: ChoiceCause,
        by: String,
    },
}
pub trait RecoveryAudit: Send + Sync {
    fn record(&self, event: &RecoveryEvent) -> Result<(), Failure>;
}

/// Which cloud new boxes are made on.
#[async_trait]
pub trait CloudChoice: Send + Sync {
    async fn read(&self) -> Result<Cloud, Failure>;
    async fn save(&self, cloud: Cloud) -> Result<(), Failure>;
}
/// The one cloud a gateway that offers no choice makes VMs on.
struct OnlyCloud(Cloud);
#[async_trait]
impl CloudChoice for OnlyCloud {
    async fn read(&self) -> Result<Cloud, Failure> {
        Ok(self.0)
    }
    async fn save(&self, cloud: Cloud) -> Result<(), Failure> {
        if cloud == self.0 {
            Ok(())
        } else {
            Err(Failure::InvalidInput)
        }
    }
}

/// Makes the connection that enrolls this install in an Agent37 workspace
/// opened by `key`: an SSH key made for it, and the template release this
/// build makes VMs from. Saving it registers the SSH key with the workspace;
/// a key saved before stays registered there until removed in Agent37.
#[async_trait]
pub trait Agent37Enrollment: Send + Sync {
    async fn connection(
        &self,
        key: zeroize::Zeroizing<String>,
    ) -> Result<crate::workspaces::domain::Agent37Connection, Failure>;
}

/// One cloud's parts: its saved connection and its VMs.
#[derive(Clone)]
struct CloudParts {
    cloud: Cloud,
    settings: Arc<dyn Settings>,
    provider: Arc<dyn Provider>,
}

/// `parts`' saved connection, which must be to that cloud.
async fn read_connection(parts: &CloudParts) -> Result<CloudConnection, Failure> {
    let connection = parts.settings.read().await?;
    if connection.cloud() != parts.cloud {
        return Err(Failure::Storage);
    }
    Ok(connection)
}

#[derive(Clone)]
pub struct Workspaces {
    clouds: Arc<Vec<CloudParts>>,
    choice: Arc<dyn CloudChoice>,
    /// Orders choosing a cloud against disconnecting one.
    choosing: Arc<tokio::sync::Mutex<()>>,
    agent37: Option<Arc<dyn Agent37Enrollment>>,
    recovery: Arc<dyn RecoveryAudit>,
    image: Option<String>,
}
impl Workspaces {
    pub fn new(
        settings: Arc<dyn Settings>,
        provider: Arc<dyn Provider>,
        recovery: Arc<dyn RecoveryAudit>,
    ) -> Self {
        Self {
            image: None,
            clouds: Arc::new(vec![CloudParts {
                cloud: Cloud::Boat,
                settings,
                provider,
            }]),
            choice: Arc::new(OnlyCloud(Cloud::Boat)),
            choosing: Arc::default(),
            agent37: None,
            recovery,
        }
    }
    /// The same workspaces, also on `cloud`, with new boxes made on whichever
    /// cloud `choice` names.
    pub fn with_cloud(
        mut self,
        cloud: Cloud,
        settings: Arc<dyn Settings>,
        provider: Arc<dyn Provider>,
        choice: Arc<dyn CloudChoice>,
    ) -> Self {
        let mut clouds = (*self.clouds).clone();
        clouds.retain(|parts| parts.cloud != cloud);
        clouds.push(CloudParts {
            cloud,
            settings,
            provider,
        });
        self.clouds = Arc::new(clouds);
        self.choice = choice;
        self
    }
    fn parts(&self, cloud: Cloud) -> Result<&CloudParts, Failure> {
        self.clouds
            .iter()
            .find(|parts| parts.cloud == cloud)
            .ok_or(Failure::NotConfigured)
    }
    /// `cloud`'s saved connection, if this gateway offers that cloud.
    pub fn settings(&self, cloud: Cloud) -> Option<Arc<dyn Settings>> {
        self.parts(cloud).ok().map(|parts| parts.settings.clone())
    }
    /// The cloud new boxes are made on.
    pub async fn cloud(&self) -> Result<Cloud, Failure> {
        self.choice.read().await
    }
    /// Make new boxes on `cloud`, which must be connected.
    ///
    /// The choice is recorded, with who made it, before it is saved: a
    /// choice that could not be recorded is not made.
    pub async fn choose(&self, cloud: Cloud, by: &str) -> Result<(), Failure> {
        let _choosing = self.choosing.lock().await;
        read_connection(self.parts(cloud)?).await?;
        self.switch(cloud, ChoiceCause::Chosen, by).await
    }
    /// Make new boxes go to `after`, recorded first.
    async fn switch(&self, after: Cloud, cause: ChoiceCause, by: &str) -> Result<(), Failure> {
        let before = self.cloud().await?;
        if before == after {
            return Ok(());
        }
        self.record_recovery(&RecoveryEvent::CloudChosen {
            before,
            after,
            cause,
            by: by.to_owned(),
        })?;
        self.choice.save(after).await
    }
    /// Whether the cloud new boxes are made on is connected.
    pub async fn connected(&self) -> bool {
        match self
            .cloud()
            .await
            .and_then(|cloud| self.parts(cloud).cloned())
        {
            Ok(parts) => parts.settings.read().await.is_ok(),
            Err(_) => false,
        }
    }
    fn record_recovery(&self, event: &RecoveryEvent) -> Result<(), Failure> {
        catch_unwind(AssertUnwindSafe(|| self.recovery.record(event)))
            .unwrap_or(Err(Failure::Storage))
    }
    pub fn with_image(mut self, image: Option<&str>) -> Self {
        self.image = image.map(str::to_owned);
        self
    }
    /// Enroll in the Agent37 workspace `key` opens and save the connection,
    /// once its SSH key is registered and its template found there.
    pub async fn save_agent37_key(&self, key: zeroize::Zeroizing<String>) -> Result<(), Failure> {
        let enrollment = self.agent37.clone().ok_or(Failure::NotConfigured)?;
        let connection = enrollment.connection(key).await?;
        self.save(CloudConnection::Agent37(connection)).await
    }
    pub fn with_agent37_enrollment(mut self, enrollment: Arc<dyn Agent37Enrollment>) -> Self {
        self.agent37 = Some(enrollment);
        self
    }
    pub async fn save_token(&self, token: String) -> Result<(), Failure> {
        let image = self.image.clone().ok_or(Failure::WorkerUnavailable)?;
        self.save(CloudConnection::Boat(BoatConnection::new(
            token,
            "nessa".into(),
            image,
        )?))
        .await
    }
    /// Forget `cloud`'s connection. If new boxes were going to `cloud`, they
    /// go to Boat again (recorded first); then its connection is cleared.
    /// Boxes already made there keep running, and are managed in that
    /// cloud's own console until it is connected again.
    pub async fn disconnect(&self, cloud: Cloud, by: &str) -> Result<(), Failure> {
        let _choosing = self.choosing.lock().await;
        let parts = self.parts(cloud)?.clone();
        if cloud != Cloud::Boat && self.cloud().await? == cloud {
            self.switch(Cloud::Boat, ChoiceCause::Disconnected, by)
                .await?;
        }
        parts.settings.clear().await
    }
    pub async fn save(&self, connection: CloudConnection) -> Result<(), Failure> {
        let parts = self.parts(connection.cloud())?;
        let generation = parts.settings.generation()?;
        let spec = connection.new_vm(None, Default::default());
        parts
            .provider
            .validate(&connection.account(), &spec)
            .await?;
        parts.settings.save(generation, connection).await
    }
}
