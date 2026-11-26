//! `terminal.*`: the person's terminals (issue #1398,
//! `docs/design/terminals.md`). The manifest's grant (`credential.manage`,
//! the owner's own window) and Cedar `terminal.use` admit a method; the
//! folder comes from the channel or conversation the request names, never
//! from the request. Every method but `terminal.open` acts through this
//! connection's [`TerminalLink`], so a view is always this connection's own.
use super::{
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::conversation::application::{ConversationError, TerminalOrigin, TerminalPlace};
use crate::conversation::domain::ChannelId;
use crate::terminal::{
    GridSize, OpenRequest, ResumeRule, ResumeSettings, ResumeSettingsError, RuleAction, RuleError,
    TerminalEntry, TerminalError, TerminalKind, TerminalLink,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    TerminalCloseParams, TerminalDetachParams, TerminalForgetSavedParams, TerminalHistoryParams,
    TerminalInputParams, TerminalListResult, TerminalOpenParams, TerminalOpenResult,
    TerminalProgramStatus, TerminalRenameParams, TerminalResumeRule, TerminalResumeSettings,
    TerminalResumeState, TerminalSavedProgram, TerminalSummary, TerminalViewParams,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde::de::DeserializeOwned;

/// Answer one `terminal.*` request on the connection that holds `link`.
pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    link: Option<&TerminalLink>,
) -> OutgoingMessage {
    let Some(link) = link else {
        return failure(&frame.id, "terminal_unavailable");
    };
    let id = frame.id.clone();
    let result = match frame.method.as_str() {
        "terminal.open" => return open(state, session, frame, link).await,
        "terminal.list" => {
            return match link.list() {
                Ok(terminals) => success(
                    &id,
                    &TerminalListResult {
                        terminals: terminals.iter().map(summary).collect(),
                    },
                ),
                Err(error) => refused(&id, &error),
            }
        }
        "terminal.attach" | "terminal.focus" | "terminal.fit" => {
            let Some(params) = decode::<TerminalViewParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let Some(fit) = GridSize::new(params.cols, params.rows) else {
                return failure(&id, "invalid_request");
            };
            match frame.method.as_str() {
                "terminal.attach" => link.attach(&params.terminal_id, &params.view_id, fit),
                "terminal.focus" => link.focus(&params.terminal_id, &params.view_id, fit),
                _ => link.fit(&params.terminal_id, &params.view_id, fit),
            }
        }
        "terminal.detach" => {
            let Some(params) = decode::<TerminalDetachParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            link.detach(&params.terminal_id, &params.view_id)
        }
        "terminal.input" => {
            let Some(params) = decode::<TerminalInputParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let Ok(bytes) = STANDARD.decode(params.data.as_bytes()) else {
                return failure(&id, "invalid_request");
            };
            link.input(&params.terminal_id, bytes)
        }
        "terminal.history" => {
            let Some(params) = decode::<TerminalHistoryParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let count = u16::try_from(params.count.min(500)).unwrap_or(500);
            link.history(&params.terminal_id, &params.view_id, params.start, count)
        }
        "terminal.close" => {
            let Some(params) = decode::<TerminalCloseParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let closed = link.close(&params.terminal_id);
            if closed.is_ok() {
                tracing::info!(
                    target: "nessa::terminal",
                    terminal = %params.terminal_id,
                    principal = %link.owner().principal,
                    "terminal closed"
                );
            }
            closed
        }
        "terminal.rename" => {
            let Some(params) = decode::<TerminalRenameParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let Some(title) = crate::terminal::named(&params.title) else {
                return failure(&id, "invalid_request");
            };
            link.rename(&params.terminal_id, title)
        }
        "terminal.resumeSettings" => return resume_state(&id, link),
        "terminal.setResumeSettings" => {
            let Some(params) = decode::<TerminalResumeSettings>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let Ok(hub) = link.hub() else {
                return failure(&id, "terminal_unavailable");
            };
            return match hub.set_resume_settings(settings_from_wire(params)) {
                Ok(_) => resume_state(&id, link),
                Err(ResumeSettingsError::Unavailable) => failure(&id, "terminal_unavailable"),
                Err(ResumeSettingsError::Invalid(RuleError::TooMany)) => {
                    failure(&id, "terminal_resume_rules_limit")
                }
                // The field it refused, so Settings can say which part to fix
                // without keeping a copy of these rules.
                Err(ResumeSettingsError::Invalid(RuleError::Invalid(field))) => {
                    failure_with_details(
                        &id,
                        "terminal_resume_rule_invalid",
                        serde_json::json!({ "field": field }),
                    )
                }
                Err(ResumeSettingsError::Storage(error)) => {
                    tracing::warn!(target: "nessa::terminal", %error, "terminal resume settings not kept");
                    failure(&id, "terminal_resume_storage_unavailable")
                }
            };
        }
        "terminal.forgetSaved" => {
            let Some(params) = decode::<TerminalForgetSavedParams>(&frame) else {
                return failure(&id, "invalid_request");
            };
            let Ok(hub) = link.hub() else {
                return failure(&id, "terminal_unavailable");
            };
            return match hub.forget_saved(link.owner(), &params.terminal_id) {
                Ok(()) => resume_state(&id, link),
                Err(error) => refused(&id, &error),
            };
        }
        _ => return failure(&id, "invalid_request"),
    };
    match result {
        Ok(()) => success(&id, &serde_json::json!({})),
        Err(error) => refused(&id, &error),
    }
}

async fn open(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    link: &TerminalLink,
) -> OutgoingMessage {
    let id = frame.id.clone();
    let (Some(service), Some(host)) = (&state.conversation.service, &state.terminals.host) else {
        return failure(&id, "terminal_unavailable");
    };
    let Some(params) = decode::<TerminalOpenParams>(&frame) else {
        return failure(&id, "invalid_request");
    };
    let Some(size) = GridSize::new(params.cols, params.rows) else {
        return failure(&id, "invalid_request");
    };
    // Run names the conversation and its command; ⌘J names a channel and
    // runs no command. Anything else is not one of the two.
    let (origin, kind) = match (&params.channel_id, &params.conversation_id, &params.command) {
        (channel, None, None) => {
            let channel = match channel {
                None => None,
                Some(channel) => match ChannelId::new(channel) {
                    Ok(channel) => Some(channel),
                    Err(_) => return failure(&id, "invalid_request"),
                },
            };
            (TerminalOrigin::Channel(channel), TerminalKind::Shell)
        }
        (None, Some(conversation), Some(command)) => {
            let Ok(conversation) = ConversationId::new(conversation) else {
                return failure(&id, "invalid_request");
            };
            if command.trim().is_empty() || command.contains('\0') {
                return failure(&id, "invalid_request");
            }
            (
                TerminalOrigin::Conversation(conversation),
                TerminalKind::Run {
                    command: command.clone(),
                },
            )
        }
        _ => return failure(&id, "invalid_request"),
    };
    let caller = super::conversation::caller(session, params.request_id.clone());
    let place = match service.terminal_place(&caller, &origin).await {
        Ok(place) => place,
        Err(ConversationError::ChannelNotFound) => return failure(&id, "channel_not_found"),
        Err(ConversationError::NotFound) => return failure(&id, "conversation_not_found"),
        Err(_) => return failure(&id, "terminal_unavailable"),
    };
    let opened = match place {
        TerminalPlace::Here { folder } => {
            let Some(folder) = host.folder(&folder) else {
                tracing::warn!(
                    target: "nessa::terminal",
                    principal = %link.owner().principal,
                    channel_id = ?params.channel_id,
                    conversation_id = ?params.conversation_id,
                    reason = "terminal_folder_missing",
                    "terminal open refused"
                );
                return failure(&id, "terminal_folder_missing");
            };
            let request = OpenRequest {
                owner: link.owner().clone(),
                request_id: params.request_id,
                channel_id: params.channel_id,
                conversation_id: params.conversation_id,
                folder,
                runs_on: "here".into(),
                kind,
                size,
                reopen: None,
            };
            link.open(request, host.login_shell())
        }
    };
    match opened {
        Ok(entry) => {
            tracing::info!(
                target: "nessa::terminal",
                terminal = %entry.id,
                principal = %link.owner().principal,
                runs_on = %entry.runs_on,
                folder = %entry.folder,
                run = matches!(entry.kind, TerminalKind::Run { .. }),
                "terminal opened"
            );
            success(
                &id,
                &TerminalOpenResult {
                    terminal: summary(&entry),
                },
            )
        }
        Err(error) => refused(&id, &error),
    }
}

/// The caller's resume settings and kept programs.
fn resume_state(id: &str, link: &TerminalLink) -> OutgoingMessage {
    let Ok(hub) = link.hub() else {
        return failure(id, "terminal_unavailable");
    };
    let Some(settings) = hub.resume_settings() else {
        return failure(id, "terminal_unavailable");
    };
    success(
        id,
        &TerminalResumeState {
            settings: settings_to_wire(&settings),
            saved: hub
                .saved_programs(link.owner())
                .into_iter()
                .take(64)
                .map(|program| TerminalSavedProgram {
                    terminal_id: program.terminal_id,
                    title: program.title,
                    command: program.command,
                    folder: program.folder,
                    saved_at_ms: program.saved_at_ms,
                    comes_back: program.comes_back,
                    open: program.open,
                })
                .collect(),
        },
    )
}

fn settings_from_wire(wire: TerminalResumeSettings) -> ResumeSettings {
    ResumeSettings {
        enabled: wire.enabled,
        rules: wire
            .rules
            .into_iter()
            .map(|rule| ResumeRule {
                program: rule.program,
                folder: rule.folder,
                args: rule.args.unwrap_or_default(),
                action: if rule.action == "never" {
                    RuleAction::Never
                } else {
                    RuleAction::Allow
                },
            })
            .collect(),
    }
}

fn settings_to_wire(settings: &ResumeSettings) -> TerminalResumeSettings {
    TerminalResumeSettings {
        enabled: settings.enabled,
        rules: settings
            .rules
            .iter()
            .map(|rule| TerminalResumeRule {
                program: rule.program.clone(),
                folder: rule.folder.clone(),
                args: (!rule.args.is_empty()).then(|| rule.args.clone()),
                action: match rule.action {
                    RuleAction::Allow => "allow".into(),
                    RuleAction::Never => "never".into(),
                },
            })
            .collect(),
    }
}

fn decode<T: DeserializeOwned>(frame: &RequestFrame) -> Option<T> {
    serde_json::from_value(frame.params.clone()).ok()
}

fn refused(id: &str, error: &TerminalError) -> OutgoingMessage {
    match error {
        TerminalError::NotFound => failure(id, "terminal_not_found"),
        TerminalError::Limit => failure(id, "terminal_limit"),
        TerminalError::Invalid => failure(id, "invalid_request"),
        TerminalError::Unavailable => failure(id, "terminal_unavailable"),
        TerminalError::Spawn(reason) => {
            tracing::warn!(target: "nessa::terminal", %reason, "terminal did not start");
            failure(id, "terminal_start_failed")
        }
        TerminalError::Storage(reason) => {
            tracing::warn!(target: "nessa::terminal", %reason, "kept program not changed");
            failure(id, "terminal_resume_storage_unavailable")
        }
    }
}

/// The wire's view of one terminal.
pub(super) fn summary(entry: &TerminalEntry) -> TerminalSummary {
    TerminalSummary {
        terminal_id: entry.id.clone(),
        title: entry.title.clone(),
        kind: match entry.kind {
            TerminalKind::Shell => "shell".into(),
            TerminalKind::Run { .. } => "run".into(),
        },
        channel_id: entry.channel_id.clone(),
        conversation_id: entry.conversation_id.clone(),
        request_id: entry.request_id.clone(),
        folder: entry.folder.clone(),
        shell: entry.shell.clone(),
        runs_on: entry.runs_on.clone(),
        started_at_ms: entry.started_at_ms,
        state: if entry.ended.is_some() {
            "exited".into()
        } else {
            "running".into()
        },
        exit_status: entry.ended.and_then(|end| end.status).map(i64::from),
        ended_at_ms: entry.ended.map(|end| end.at_ms),
        resume_command: entry.resume_command.clone(),
        resumed_at_ms: entry.resumed_at_ms,
        program_status: entry
            .program_status
            .as_ref()
            .map(|stamped| TerminalProgramStatus {
                state: stamped.status.state.as_str().into(),
                kind: stamped.status.kind.map(|kind| kind.as_str().into()),
                progress: stamped.status.progress.map(u64::from),
                app: stamped.status.app.clone(),
                title: stamped.status.title.clone(),
                msg: stamped.status.msg.clone(),
                updated_at_ms: stamped.updated_at_ms,
            }),
    }
}
