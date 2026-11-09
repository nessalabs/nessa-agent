//! Fixed GitHub reads from an admitted live agent turn. The token stays here.
use crate::conversation::application::{AgentToolRequest, ConversationService};
use crate::github::application::{GithubConnections, GithubReadError, GithubReadFailure};
use crate::github::domain::{GithubReadKind, GithubRepository};
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;

const SERVER_NAME: &str = "nessa-github";
const MAX_RESULT_BYTES: usize = 32 * 1024;

pub(super) struct GithubTools {
    service: OnceLock<ConversationService>,
    connections: Arc<GithubConnections>,
}

impl GithubTools {
    pub(super) fn new(connections: Arc<GithubConnections>) -> Arc<Self> {
        Arc::new(Self {
            service: OnceLock::new(),
            connections,
        })
    }

    pub(super) fn attach(&self, service: ConversationService) {
        let _ = self.service.set(service);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Read {
    owner: String,
    repository: String,
    number: Option<u64>,
}

fn read_input(
    tool: &str,
    arguments: Value,
) -> Result<(GithubRepository, GithubReadKind), &'static str> {
    if !arguments.is_object() {
        return Err("invalid_request");
    }
    let read: Read = serde_json::from_value(arguments).map_err(|_| "invalid_request")?;
    let repository =
        GithubRepository::new(&read.owner, &read.repository).map_err(|_| "invalid_request")?;
    let kind = match (tool, read.number) {
        ("read_repository", None) => GithubReadKind::Repository,
        ("read_issue", Some(number)) => GithubReadKind::Issue(number),
        ("read_pull_request", Some(number)) => GithubReadKind::PullRequest(number),
        _ => return Err("invalid_request"),
    };
    repository.path(kind).map_err(|_| "invalid_request")?;
    Ok((repository, kind))
}

fn failed(code: &str) -> Value {
    let value = json!({"error":code});
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}

fn answer(value: Value) -> Value {
    let text = value.to_string();
    if text.len() > MAX_RESULT_BYTES {
        return failed("github_answer_too_large");
    }
    json!({"content":[{"type":"text","text":text}],"structuredContent":value,"isError":false})
}

fn error(error: GithubReadError) -> &'static str {
    match error {
        GithubReadError::Cancelled => "github_call_cancelled",
        GithubReadError::NotConnected => "github_not_connected",
        GithubReadError::Api(GithubReadFailure::NotFound) => "github_not_found",
        GithubReadError::Api(GithubReadFailure::Unauthorized) => "github_unauthorized",
        GithubReadError::Api(GithubReadFailure::Unreachable) => "github_unreachable",
        GithubReadError::Api(GithubReadFailure::InvalidAnswer) => "github_invalid_answer",
    }
}

impl BuiltInServer for GithubTools {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn configuration(&self) -> Vec<String> {
        vec!["github-read/1".into()]
    }

    fn tools(&self) -> Vec<Value> {
        [
            (
                "read_repository",
                "Read one GitHub repository's metadata with the current Nessa sign-in.",
                false,
            ),
            (
                "read_issue",
                "Read one GitHub issue with the current Nessa sign-in.",
                true,
            ),
            (
                "read_pull_request",
                "Read one GitHub pull request with the current Nessa sign-in.",
                true,
            ),
        ]
        .into_iter()
        .map(|(name, description, numbered)| {
            let mut properties = json!({
                "owner": {"type":"string"},
                "repository": {"type":"string"},
            });
            let mut required = vec!["owner", "repository"];
            if numbered {
                properties["number"] = json!({"type":"integer","minimum":1});
                required.push("number");
            }
            json!({
                "name": name,
                "description": description,
                "inputSchema": {
                    "type":"object",
                    "properties":properties,
                    "required":required,
                    "additionalProperties":false,
                },
                "annotations":{"readOnlyHint":true,"openWorldHint":true},
            })
        })
        .collect()
    }

    fn call(
        &self,
        session: &SessionId,
        call: String,
        tool: String,
        arguments: Value,
        stop: watch::Receiver<bool>,
    ) -> BuiltInFuture {
        let service = self.service.get().cloned();
        let source = ConversationId::new(session.as_str());
        let connections = self.connections.clone();
        Box::pin(async move {
            let result = async {
                let (Some(service), Ok(source)) = (service, source) else {
                    return failed("github_tools_unavailable");
                };
                if call.is_empty() || call.len() > 256 {
                    return failed("invalid_request");
                }
                let (repository, kind) = match read_input(&tool, arguments) {
                    Ok(input) => input,
                    Err(code) => return failed(code),
                };
                let resource = match repository.path(kind) {
                    Ok(path) => path,
                    Err(_) => return failed("invalid_request"),
                };
                // The relay fixes the source session and call. The caller
                // cannot supply an actor or an authority claim in arguments.
                if service
                    .admit_agent_tool(
                        source,
                        call.clone(),
                        format!("{SERVER_NAME}.{tool}:{resource}"),
                        None,
                        AgentToolRequest {
                            logical: call.clone(),
                            command: uuid::Uuid::new_v4().to_string(),
                        },
                        stop.clone(),
                    )
                    .await
                    .is_err()
                {
                    return failed("github_source_refused");
                }
                match connections.read_repository(&repository, kind, stop).await {
                    Ok(value) => answer(value),
                    Err(problem) => failed(error(problem)),
                }
            }
            .await;
            result.into()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fixed_repository_reads_are_parsed() {
        let repo = json!({"owner":"nessalabs","repository":"nessa-agent"});
        assert!(matches!(
            read_input("read_repository", repo.clone()),
            Ok((_, GithubReadKind::Repository))
        ));
        assert!(read_input("read_issue", repo.clone()).is_err());
        assert!(read_input("read_repository", json!({"owner":"a/b","repository":"c"})).is_err());
        assert!(read_input(
            "read_repository",
            json!({"owner":"a","repository":"b","number":1})
        )
        .is_err());
        assert!(matches!(
            read_input(
                "read_issue",
                json!({"owner":"a","repository":"b","number":1})
            ),
            Ok((_, GithubReadKind::Issue(1)))
        ));
        assert!(read_input(
            "read_pull_request",
            json!({"owner":"a","repository":"b","number":0})
        )
        .is_err());
        assert!(read_input("unknown", repo).is_err());
    }
}
