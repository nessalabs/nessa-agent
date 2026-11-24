//! The bytes of the vault entry `login.codex.chatgpt`: [`ChatgptAccounts`]
//! as JSON. The domain keeps no serialization; this is its one form.
use super::domain::{AccountId, ChatgptAccount, ChatgptAccounts, Secret, Tokens, Unreadable};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    host_id: String,
    in_use: Option<String>,
    accounts: Vec<Account>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Account {
    id: String,
    email: String,
    subject: String,
    client_id: String,
    tokens: Option<EntryTokens>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntryTokens {
    access: String,
    refresh: String,
    expires_at_ms: u64,
    earliest_refresh_at_ms: Option<u64>,
}

impl Drop for EntryTokens {
    fn drop(&mut self) {
        self.access.zeroize();
        self.refresh.zeroize();
    }
}

/// The entry's bytes for `accounts`.
pub fn encode(accounts: &ChatgptAccounts) -> Vec<u8> {
    let entry = Entry {
        host_id: accounts.host_id().to_owned(),
        in_use: accounts.in_use_id().map(|id| id.as_str().to_owned()),
        accounts: accounts
            .accounts()
            .iter()
            .map(|account| Account {
                id: account.id.as_str().to_owned(),
                email: account.email.clone(),
                subject: account.subject.clone(),
                client_id: account.client_id.clone(),
                tokens: account.tokens.as_ref().map(|tokens| EntryTokens {
                    access: tokens.access.expose().to_owned(),
                    refresh: tokens.refresh.expose().to_owned(),
                    expires_at_ms: tokens.expires_at_ms,
                    earliest_refresh_at_ms: tokens.earliest_refresh_at_ms,
                }),
            })
            .collect(),
    };
    serde_json::to_vec(&entry).unwrap_or_default()
}

/// The accounts in an entry's bytes.
///
/// # Errors
/// [`Unreadable`] for anything [`encode`] did not write, or accounts the
/// domain does not keep (see [`ChatgptAccounts::from_parts`]).
pub fn decode(bytes: &[u8]) -> Result<ChatgptAccounts, Unreadable> {
    let entry: Entry = serde_json::from_slice(bytes).map_err(|_| Unreadable)?;
    let id = |value: &str| AccountId::parse(value).map_err(|_| Unreadable);
    let in_use = entry.in_use.as_deref().map(id).transpose()?;
    let accounts = entry
        .accounts
        .iter()
        .map(|account| {
            Ok(ChatgptAccount {
                id: id(&account.id)?,
                email: account.email.clone(),
                subject: account.subject.clone(),
                client_id: account.client_id.clone(),
                tokens: account.tokens.as_ref().map(|tokens| Tokens {
                    access: Secret::new(tokens.access.clone()),
                    refresh: Secret::new(tokens.refresh.clone()),
                    expires_at_ms: tokens.expires_at_ms,
                    earliest_refresh_at_ms: tokens.earliest_refresh_at_ms,
                }),
            })
        })
        .collect::<Result<Vec<_>, Unreadable>>()?;
    ChatgptAccounts::from_parts(entry.host_id.clone(), in_use, accounts)
}
