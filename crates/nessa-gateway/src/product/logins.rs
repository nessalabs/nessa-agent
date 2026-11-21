//! The saved-login methods (issue #1260): `logins.status`, `logins.save`,
//! `logins.remove` and `logins.setDevice`, and Codex's ChatGPT accounts'
//! `logins.chatgptSignIn`, `logins.chatgptCancel`, `logins.chatgptUse` and
//! `logins.chatgptRemove` (`docs/design/chatgpt-sign-in.md`), each admitted
//! by Cedar's `credential.manage`, translated into [`SavedLoginsService`].
//! No answer carries a login or a token.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::logins::application::{LoginCaller, LoginsError, LoginsStatus};
use crate::logins::chatgpt::{
    application::{ChatgptStatus, SignInStatus},
    domain::{AccountId, SignInEnd},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::agents::AgentId;
use nessa_protocol::lease::Login;
use nessa_protocol::product::generated::{
    ChatgptAccount as WireAccount, ChatgptLogins as WireChatgpt, ChatgptSignIn as WireSignIn,
    ChatgptSignInEnd as WireEnd, LoginAgent, LoginsChatgptRemoveParams, LoginsChatgptSignInParams,
    LoginsChatgptUseParams, LoginsErrorCode, LoginsRemoveParams, LoginsSaveParams,
    LoginsSetDeviceParams, LoginsStatus as WireStatus, SavedLogin,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(logins) = state.accounts.logins.as_ref() else {
        return failure(&frame.id, LoginsErrorCode::LoginsNotConfigured.as_str());
    };
    let caller = caller(session);
    let answer = match frame.method.as_str() {
        "logins.status" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            logins.status()
        }
        "logins.save" => {
            let Ok(params) = serde_json::from_value::<LoginsSaveParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(login) = Login::new(params.login) else {
                return failure(&frame.id, "invalid_request");
            };
            logins.save(caller, agent(params.agent), login).await
        }
        "logins.remove" => {
            let Ok(params) = serde_json::from_value::<LoginsRemoveParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            logins.remove(caller, agent(params.agent)).await
        }
        "logins.setDevice" => {
            let Ok(params) = serde_json::from_value::<LoginsSetDeviceParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(peer) = PeerEnvironment::parse(&params.environment) else {
                return failure(&frame.id, "invalid_request");
            };
            logins.set_device(caller, peer, params.on).await
        }
        "logins.chatgptSignIn" => {
            let Ok(params) =
                serde_json::from_value::<LoginsChatgptSignInParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(account) = params
                .account_id
                .as_deref()
                .map(AccountId::parse)
                .transpose()
            else {
                return failure(&frame.id, "invalid_request");
            };
            let begun = match logins.chatgpt() {
                Ok(chatgpt) => chatgpt.begin(caller, account).await,
                Err(error) => Err(error),
            };
            // The answer names the attempt this call started: the status read
            // after it may already show another window's.
            return match begun.and_then(|started| Ok((started, logins.status()?))) {
                Ok((started, status)) => {
                    let mut answer = wire(&status);
                    if let Some(chatgpt) = answer.chatgpt.as_mut() {
                        chatgpt.started = Some(started);
                    }
                    success(&frame.id, &answer)
                }
                Err(error) => failure(&frame.id, code(error).as_str()),
            };
        }
        "logins.chatgptCancel" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            match logins.chatgpt() {
                Ok(chatgpt) => {
                    chatgpt.cancel(&caller);
                    logins.status()
                }
                Err(error) => Err(error),
            }
        }
        "logins.chatgptUse" => {
            let Ok(params) = serde_json::from_value::<LoginsChatgptUseParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(account) = params
                .account_id
                .as_deref()
                .map(AccountId::parse)
                .transpose()
            else {
                return failure(&frame.id, "invalid_request");
            };
            match logins.chatgpt() {
                Ok(chatgpt) => match chatgpt.use_account(caller, account).await {
                    Ok(()) => logins.status(),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            }
        }
        "logins.chatgptRemove" => {
            let Ok(params) =
                serde_json::from_value::<LoginsChatgptRemoveParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(account) = AccountId::parse(&params.account_id) else {
                return failure(&frame.id, "invalid_request");
            };
            match logins.chatgpt() {
                Ok(chatgpt) => match chatgpt.remove(caller, account).await {
                    Ok(()) => logins.status(),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            }
        }
        _ => return failure(&frame.id, "unknown_method"),
    };
    match answer {
        Ok(status) => success(&frame.id, &wire(&status)),
        Err(error) => failure(&frame.id, code(error).as_str()),
    }
}

fn agent(agent: LoginAgent) -> AgentId {
    match agent {
        LoginAgent::Claude => AgentId::Claude,
        LoginAgent::Codex => AgentId::Codex,
        LoginAgent::Opencode => AgentId::Opencode,
    }
}

fn wire_agent(agent: AgentId) -> LoginAgent {
    match agent {
        AgentId::Claude => LoginAgent::Claude,
        AgentId::Codex => LoginAgent::Codex,
        AgentId::Opencode => LoginAgent::Opencode,
    }
}

fn wire(status: &LoginsStatus) -> WireStatus {
    WireStatus {
        logins: AgentId::ALL
            .iter()
            .map(|agent| SavedLogin {
                agent: wire_agent(*agent),
                saved: status.saved.get(agent).copied().unwrap_or(false),
            })
            .collect(),
        devices_on: status
            .devices_on
            .iter()
            .map(PeerEnvironment::name)
            .collect(),
        chatgpt: status.chatgpt.as_ref().map(wire_chatgpt),
    }
}

fn wire_chatgpt(status: &ChatgptStatus) -> WireChatgpt {
    WireChatgpt {
        started: None,
        accounts: status
            .accounts
            .iter()
            .map(|account| WireAccount {
                id: account.id.as_str().to_owned(),
                email: account.email.clone(),
                in_use: account.in_use,
                needs_sign_in: account.needs_sign_in,
            })
            .collect(),
        sign_in: match &status.sign_in {
            SignInStatus::Idle { last, attempt } => WireSignIn {
                state: "idle".into(),
                attempt: attempt.clone(),
                authorize_url: None,
                account_id: None,
                last: last.map(wire_end),
            },
            SignInStatus::Waiting {
                attempt,
                authorize_url,
                account,
            } => WireSignIn {
                state: "waiting".into(),
                attempt: Some(attempt.clone()),
                authorize_url: Some(authorize_url.clone()),
                account_id: account.as_ref().map(|account| account.as_str().to_owned()),
                last: None,
            },
        },
    }
}

fn wire_end(end: SignInEnd) -> WireEnd {
    match end {
        SignInEnd::SignedIn => WireEnd::SignedIn,
        SignInEnd::Declined => WireEnd::Declined,
        SignInEnd::Incomplete => WireEnd::Incomplete,
        SignInEnd::WrongAccount => WireEnd::WrongAccount,
        SignInEnd::ExchangeFailed => WireEnd::ExchangeFailed,
        SignInEnd::InvalidIdentity => WireEnd::InvalidIdentity,
        SignInEnd::PlanNotEnabled => WireEnd::PlanNotEnabled,
        SignInEnd::TooManyAccounts => WireEnd::TooManyAccounts,
        SignInEnd::NotSaved => WireEnd::NotSaved,
        SignInEnd::TimedOut => WireEnd::TimedOut,
        SignInEnd::Cancelled => WireEnd::Cancelled,
        SignInEnd::Replaced => WireEnd::Replaced,
        SignInEnd::ListenFailed => WireEnd::ListenFailed,
    }
}

pub(super) fn caller(session: &AuthenticatedSession) -> LoginCaller {
    let context = session.context();
    LoginCaller {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn code(error: LoginsError) -> LoginsErrorCode {
    match error {
        LoginsError::VaultLocked => LoginsErrorCode::VaultLocked,
        LoginsError::StorageUnavailable => LoginsErrorCode::LoginsStorageUnavailable,
        LoginsError::AuditUnavailable => LoginsErrorCode::LoginsAuditUnavailable,
        LoginsError::ChangeUnrecorded => LoginsErrorCode::LoginsChangeUnrecorded,
        LoginsError::TooManyDevices => LoginsErrorCode::LoginsTooManyDevices,
        LoginsError::UnknownAccount => LoginsErrorCode::LoginsUnknownAccount,
        LoginsError::ChatgptNotConfigured => LoginsErrorCode::LoginsChatgptNotConfigured,
    }
}
