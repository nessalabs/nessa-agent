//! Strict provider receipts. No SDK/SSH/MCP authority follows an unverified VM.
use crate::workspaces::application::WorkspaceReceipt;
use crate::workspaces::domain::{
    AdmissionRefusal, AdmissionRejection, BoatConnection, EnvironmentConfiguration,
    EnvironmentIdentity, Failure, ObservedEnvironment, RefusalDisposition,
};
use serde_json::Value;
use workspace_sdk::boat_client::{
    Api, CreateSandboxRequest, CreateSandboxResponse, SandboxInfoResponse, SandboxState,
};

pub(super) async fn environment(
    api: &Api,
    connection: &BoatConnection,
) -> Result<EnvironmentIdentity, Failure> {
    let response = api.environments().await.map_err(|_| Failure::Provider)?;
    if response.extra.len() != 2
        || response.extra.get("ok") != Some(&Value::Bool(true))
        || response.extra.get("type").and_then(Value::as_str) != Some("environments.list")
    {
        return Err(Failure::EnvironmentUnavailable);
    }
    let mut matches = response
        .environments
        .iter()
        .filter(|e| e.name == connection.environment);
    let env = matches.next().ok_or(Failure::EnvironmentUnavailable)?;
    if matches.next().is_some() {
        return Err(Failure::EnvironmentUnavailable);
    }
    let proxy_ai_credentials = env
        .extra
        .get("proxyAiCredentials")
        .and_then(Value::as_bool)
        .ok_or(Failure::EnvironmentUnavailable)?;
    EnvironmentConfiguration {
        safe_for_third_parties: env.safe_for_third_parties,
        pass_github: env.pass_github,
        pass_secrets: env.pass_secrets,
        pass_sandbox_credentials: env.pass_sandbox_credentials,
        pass_agents_credentials: env.pass_agents_credentials,
        has_contents: !env.env_contents.is_empty(),
        has_secret_files: !env.secret_files.is_empty(),
        has_repositories: env
            .selected_repositories
            .as_ref()
            .is_some_and(|r| !r.is_empty()),
        owner_default_profile: env.extra.get("credentialProfileId") == Some(&Value::Null),
        proxy_ai_credentials,
    }
    .admit()?;
    // Boat's published contract: explicit null chooses the owner's default;
    // proxyAiCredentials applies only to safe-for-third-parties environments.
    // Personal-scope ResourceAccess is an object, unlike a VM's string access.
    // Verify the provider's computed flags agree with its declared configuration.
    if env.extra.get("access") != Some(&serde_json::json!({"restricted":false,"members":[]}))
        || env.extra.get("effective")
            != Some(&serde_json::json!({
                "passGithub":env.pass_github,"passSecrets":env.pass_secrets,
                "passSandboxCredentials":env.pass_sandbox_credentials,"passAgentsCredentials":env.pass_agents_credentials
            }))
        || env.extra.keys().any(|key| {
            !matches!(
                key.as_str(),
                "credentialProfileId"
                    | "proxyAiCredentials"
                    | "access"
                    | "effective"
                    | "latestVersionNumber"
                    | "safetyAnsweredAt"
            )
        })
        || env.versions.iter().any(|v| !v.extra.is_empty())
    {
        return Err(Failure::EnvironmentUnavailable);
    }
    if env.extra.get("safetyAnsweredAt").is_some_and(|v| {
        !v.is_null()
            && !v
                .as_str()
                .is_some_and(|s| s.len() <= 64 && !s.chars().any(char::is_control))
    }) {
        return Err(Failure::EnvironmentUnavailable);
    }
    let id = env
        .latest_version_id
        .as_ref()
        .ok_or(Failure::EnvironmentUnavailable)?;
    let mut versions = env.versions.iter().filter(|v| &v.id == id);
    let version = versions.next().ok_or(Failure::EnvironmentUnavailable)?;
    if versions.next().is_some()
        || env
            .versions
            .iter()
            .filter(|v| v.version_number == version.version_number)
            .count()
            != 1
        || env.extra.get("latestVersionNumber").and_then(Value::as_u64)
            != Some(version.version_number)
    {
        return Err(Failure::EnvironmentUnavailable);
    }
    EnvironmentIdentity::new(
        env.id.clone(),
        env.name.clone(),
        id.clone(),
        u32::try_from(version.version_number).map_err(|_| Failure::EnvironmentUnavailable)?,
    )
}

fn supported_state(sandbox: &workspace_sdk::boat_client::Sandbox) -> bool {
    sandbox.state != SandboxState::Unknown
}

async fn bounded_json(mut response: reqwest::Response) -> Result<Value, Failure> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Provider)? {
        if chunk.len() > 65_536 - bytes.len() {
            return Err(Failure::AdmissionRejected(AdmissionRejection::new(
                AdmissionRefusal::InvalidReceipt,
                None,
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        Failure::AdmissionRejected(AdmissionRejection::new(
            AdmissionRefusal::InvalidReceipt,
            None,
        ))
    })?;
    Ok(value)
}

pub(super) async fn create(
    http: &reqwest::Client,
    api: &Api,
    connection: &BoatConnection,
    thread: &str,
    admission: EnvironmentIdentity,
    ttl_seconds: Option<u32>,
) -> Result<WorkspaceReceipt, Failure> {
    let response = http
        .post(format!(
            "{}/sandboxes",
            api.config().base_path.trim_end_matches('/')
        ))
        .bearer_auth(&api.config().access_token)
        .header("X-Boat-Org", "personal")
        .header("Idempotency-Key", format!("nessa-thread-{thread}"))
        .json(&CreateSandboxRequest {
            type_: Some("small".into()),
            ttl_seconds: Some(ttl_seconds),
            environment: Some(connection.environment.clone()),
            env: Some(std::collections::HashMap::from([(
                "NESSA_THREAD_ID".into(),
                thread.into(),
            )])),
            from: Some(connection.image.clone()),
            snapshots: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .map_err(|_| Failure::ProvisioningUncertain)?;
    if !response.status().is_success() {
        return Err(Failure::ProvisioningUncertain);
    }
    let value = bounded_json(response)
        .await
        .map_err(|_| Failure::ProvisioningUncertain)?;
    // A ready response is an idempotent replay of an older VM. An unknown
    // acknowledgement cannot authorize deletion of the ID it happens to name.
    let acknowledgement = match (
        value.get("ok").and_then(Value::as_bool),
        value.get("type").and_then(Value::as_str),
        value.get("status").and_then(Value::as_str),
    ) {
        (Some(true), Some("sandbox.created"), Some("provisioning")) => {
            Some(RefusalDisposition::RollbackNewVm)
        }
        (Some(true), Some("sandbox.created"), Some("ready")) => {
            Some(RefusalDisposition::RetainTarget)
        }
        _ => None,
    };
    let disposition = acknowledgement.unwrap_or(RefusalDisposition::RetainTarget);
    let vm = value
        .get("sandbox")
        .and_then(|s| s.get("id"))
        .and_then(Value::as_str)
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
        .ok_or(Failure::ProvisioningUncertain)?
        .to_owned();
    let created: CreateSandboxResponse = match serde_json::from_value(value) {
        Ok(created) => created,
        Err(_) => {
            return Ok(WorkspaceReceipt {
                vm,
                admission,
                disposition,
                refusal: Some(AdmissionRejection::new(
                    AdmissionRefusal::InvalidReceipt,
                    None,
                )),
            })
        }
    };
    let observation = observed(&created.sandbox);
    let mut refusal = admission
        .check(&observation)
        .err()
        .map(|cause| AdmissionRejection::new(cause, Some(observation.clone())));
    if acknowledgement.is_none() || !supported_state(&created.sandbox) {
        refusal = Some(AdmissionRejection::new(
            AdmissionRefusal::InvalidReceipt,
            Some(observation),
        ));
    }
    if refusal.is_none() {
        refusal = match check(http, api, connection, &vm, &admission, ttl_seconds).await {
            Ok(_) => None,
            Err(Failure::AdmissionRejected(rejection)) => Some(rejection),
            Err(_) => Some(AdmissionRejection::new(
                AdmissionRefusal::ReceiptUnavailable,
                None,
            )),
        };
    }
    Ok(WorkspaceReceipt {
        vm,
        admission,
        refusal,
        disposition,
    })
}

/// The SDK's Sandbox decoder ignores unknown fields. Read the actual JSON once
/// through the injected client so new credential/access fields cannot disappear.
pub(super) async fn receipt(
    http: &reqwest::Client,
    api: &Api,
    vm: &str,
    maximum_ttl: Option<u32>,
) -> Result<SandboxInfoResponse, Failure> {
    let response = http
        .get(format!(
            "{}/sandboxes/{vm}",
            api.config().base_path.trim_end_matches('/')
        ))
        .bearer_auth(&api.config().access_token)
        .header("X-Boat-Org", "personal")
        .send()
        .await
        .map_err(|_| Failure::Provider)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Failure::Missing);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(Failure::Provider);
    }
    let value = bounded_json(response).await?;
    let envelope = value
        .as_object()
        .ok_or(Failure::AdmissionRejected(AdmissionRejection::new(
            AdmissionRefusal::InvalidReceipt,
            None,
        )))?;
    let sandbox =
        value
            .get("sandbox")
            .and_then(Value::as_object)
            .ok_or(Failure::AdmissionRejected(AdmissionRejection::new(
                AdmissionRefusal::InvalidReceipt,
                None,
            )))?;
    let known = [
        "id",
        "name",
        "state",
        "error",
        "dataExpiredAt",
        "stoppedAt",
        "labels",
        "health",
        "healthReason",
        "degradedSince",
        "hydrated",
        "hydratedAt",
        "hydration",
        "type",
        "vcpu",
        "memoryGB",
        "billingMultiplier",
        "machineProvider",
        "url",
        "ip",
        "sshEndpoint",
        "createdAt",
        "updatedAt",
        "archiveAfter",
        "ttlSeconds",
        "desktopAvailable",
        "desktopUrl",
        "snapshots",
        "monitoring",
        "snapshotAvailable",
        "snapshotCompletedAt",
        "snapshotVerifiedAt",
        "lastSnapshotAttemptAt",
        "lastSnapshotStatus",
        "lastReboot",
        "subdomain",
        "team",
        "createdBy",
        "createdById",
        "access",
        "holdsCreatorLogins",
        "visibility",
        "wipePendingUntilRestart",
        "setupStatus",
        "setupError",
        "environment",
        "environmentVersion",
        "stop",
    ];
    if envelope
        .keys()
        .any(|k| !matches!(k.as_str(), "ok" | "type" | "sandbox" | "giftLimitNotice"))
        || value.get("ok") != Some(&Value::Bool(true))
        || value.get("type").and_then(Value::as_str) != Some("sandbox.info")
        || sandbox.keys().any(|k| !known.contains(&k.as_str()))
        || sandbox.get("id").and_then(Value::as_str) != Some(vm)
        || sandbox.get("access").and_then(Value::as_str) != Some("owner")
        || sandbox.get("team") != Some(&Value::Null)
        || sandbox.get("visibility") != Some(&Value::Null)
        || sandbox
            .get("holdsCreatorLogins")
            .and_then(Value::as_bool)
            .is_none()
        || sandbox.get("wipePendingUntilRestart") != Some(&Value::Bool(false))
        || sandbox.get("setupStatus") != Some(&Value::Null)
        || sandbox.get("setupError") != Some(&Value::Null)
    {
        return Err(Failure::AdmissionRejected(AdmissionRejection::new(
            AdmissionRefusal::InvalidReceipt,
            None,
        )));
    }
    // A finite choice is an upper bound. Boat may impose a shorter account
    // limit, but null, missing or longer provider TTL cannot satisfy it.
    if let Some(maximum) = maximum_ttl {
        let observed = sandbox
            .get("ttlSeconds")
            .and_then(Value::as_u64)
            .and_then(|seconds| u32::try_from(seconds).ok());
        if !observed.is_some_and(|seconds| seconds > 0 && seconds <= maximum) {
            return Err(Failure::AdmissionRejected(AdmissionRejection::new(
                AdmissionRefusal::LifetimeNotEnforced,
                None,
            )));
        }
    }
    let decoded: SandboxInfoResponse = serde_json::from_value(value).map_err(|_| {
        Failure::AdmissionRejected(AdmissionRejection::new(
            AdmissionRefusal::InvalidReceipt,
            None,
        ))
    })?;
    if !supported_state(&decoded.sandbox) {
        return Err(Failure::AdmissionRejected(AdmissionRejection::new(
            AdmissionRefusal::InvalidReceipt,
            Some(observed(&decoded.sandbox)),
        )));
    }
    Ok(decoded)
}
pub(super) fn observed(sandbox: &workspace_sdk::boat_client::Sandbox) -> ObservedEnvironment {
    ObservedEnvironment {
        name: sandbox.environment.clone(),
        version: sandbox.environment_version,
    }
}
pub(super) async fn check(
    http: &reqwest::Client,
    api: &Api,
    connection: &BoatConnection,
    vm: &str,
    admitted: &EnvironmentIdentity,
    maximum_ttl: Option<u32>,
) -> Result<SandboxInfoResponse, Failure> {
    let current = environment(api, connection)
        .await
        .map_err(|error| match error {
            Failure::EnvironmentUnavailable => Failure::AdmissionRejected(AdmissionRejection::new(
                AdmissionRefusal::EnvironmentUnavailable,
                None,
            )),
            other => other,
        })?;
    if &current != admitted {
        return Err(Failure::AdmissionRejected(AdmissionRejection::new(
            AdmissionRefusal::EnvironmentChanged,
            None,
        )));
    }
    let found = receipt(http, api, vm, maximum_ttl).await?;
    admitted.check(&observed(&found.sandbox)).map_err(|cause| {
        Failure::AdmissionRejected(AdmissionRejection::new(
            cause,
            Some(observed(&found.sandbox)),
        ))
    })?;
    Ok(found)
}
