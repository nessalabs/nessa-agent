use crate::conversation::domain::ConversationId;
use crate::read_only_sync::application::{CacheError, GatewayError};
use crate::read_only_sync::domain::CacheReset;
use nessa_sync::replication::catalogue::EntryKey;
use nessa_sync::replication::domain::{Id, Scope};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::path::PathBuf;

pub(crate) const HELP: &str = "read_only_sync sync-records CACHE PROFILE CONVERSATION PAGES\nread_only_sync check-records CACHE PROFILE CONVERSATION\nread_only_sync sync-catalogue CACHE PROFILE PAGES\nread_only_sync list CACHE RECEIVER ORIGIN CATALOGUE [AFTER_CREATION AFTER_ID]\nread_only_sync show CACHE RECEIVER ORIGIN CONVERSATION\nread_only_sync reset-records|reset-catalogue CACHE RECEIVER ORIGIN STREAM OPERATION CALLER GENERATION OLD_INCARNATION OLD_SCHEMA OLD_EPOCH NEW_INCARNATION NEW_SCHEMA NEW_EPOCH";

pub(crate) enum Command {
    Local(LocalCommand),
    Records {
        cache: PathBuf,
        profile: PathBuf,
        conversation: ConversationId,
        pages: usize,
    },
    Catalogue {
        cache: PathBuf,
        profile: PathBuf,
        pages: usize,
    },
}

pub(crate) struct LocalCommand {
    pub(crate) cache: PathBuf,
    pub(crate) receiver: Id,
    pub(crate) origin: Id,
    pub(crate) operation: Operation,
}
pub(crate) enum Operation {
    List { stream: Id, after: Option<EntryKey> },
    Show(ConversationId),
    ResetRecords(Box<CacheReset>),
    ResetCatalogue(Box<CacheReset>),
}

#[derive(Debug)]
pub(crate) enum CommandError {
    Arguments,
    Identity,
    Cache(CacheError),
    Output,
    Gateway(GatewayError),
    OnlineRefused,
}
impl Display for CommandError {
    fn fmt(&self, out: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Arguments => out.write_str(HELP),
            Self::Identity => out.write_str("invalid saved target identity"),
            Self::Cache(error) => error.fmt(out),
            Self::Output => out.write_str("command output unavailable"),
            Self::Gateway(error) => write!(out, "gateway {error:?}"),
            Self::OnlineRefused => out.write_str("online command refused; see JSON outcome"),
        }
    }
}

pub(crate) fn parse(args: &[String]) -> Result<Command, CommandError> {
    match args {
        [name, cache, profile, conversation, pages] if name == "sync-records" => {
            return Ok(Command::Records {
                cache: cache.into(),
                profile: profile.into(),
                conversation: ConversationId::new(conversation)
                    .map_err(|_| CommandError::Identity)?,
                pages: page_count(pages)?,
            })
        }
        [name, cache, profile, conversation] if name == "check-records" => {
            return Ok(Command::Records {
                cache: cache.into(),
                profile: profile.into(),
                conversation: ConversationId::new(conversation)
                    .map_err(|_| CommandError::Identity)?,
                pages: 0,
            })
        }
        [name, cache, profile, pages] if name == "sync-catalogue" => {
            return Ok(Command::Catalogue {
                cache: cache.into(),
                profile: profile.into(),
                pages: page_count(pages)?,
            })
        }
        _ => {}
    }
    parse_local(args).map(Command::Local)
}

fn parse_local(args: &[String]) -> Result<LocalCommand, CommandError> {
    if matches!(
        args.first().map(String::as_str),
        Some("reset-records" | "reset-catalogue")
    ) {
        return parse_reset(args);
    }
    let (name, cache, receiver, origin, target, continuation) = match args {
        [name, cache, receiver, origin, target] => (name, cache, receiver, origin, target, None),
        [name, cache, receiver, origin, target, creation, id] if name == "list" => {
            (name, cache, receiver, origin, target, Some((creation, id)))
        }
        _ => return Err(CommandError::Arguments),
    };
    let operation = match name.as_str() {
        "list" => Operation::List {
            stream: Id::new(target).map_err(|_| CommandError::Identity)?,
            after: continuation
                .map(|(creation, id)| {
                    Ok(EntryKey {
                        creation: creation.parse().map_err(|_| CommandError::Arguments)?,
                        id: Id::new(id).map_err(|_| CommandError::Identity)?,
                    })
                })
                .transpose()?,
        },
        "show" => Operation::Show(ConversationId::new(target).map_err(|_| CommandError::Identity)?),
        _ => return Err(CommandError::Arguments),
    };
    Ok(LocalCommand {
        cache: PathBuf::from(cache),
        receiver: Id::new(receiver).map_err(|_| CommandError::Identity)?,
        origin: Id::new(origin).map_err(|_| CommandError::Identity)?,
        operation,
    })
}

fn parse_reset(args: &[String]) -> Result<LocalCommand, CommandError> {
    let [name, cache, receiver, origin, stream, operation, caller, generation, old_incarnation, old_schema, old_epoch, new_incarnation, new_schema, new_epoch] =
        args
    else {
        return Err(CommandError::Arguments);
    };
    let receiver = identity(receiver)?;
    let origin = identity(origin)?;
    let stream = identity(stream)?;
    let expected = Scope::new(
        receiver.clone(),
        origin.clone(),
        stream.clone(),
        identity(old_incarnation)?,
        identity(old_schema)?,
        identity(old_epoch)?,
    );
    let replacement = Scope::new(
        receiver.clone(),
        origin.clone(),
        stream,
        identity(new_incarnation)?,
        identity(new_schema)?,
        identity(new_epoch)?,
    );
    let request = Box::new(
        CacheReset::new(
            identity(operation)?,
            identity(caller)?,
            expected,
            generation.parse().map_err(|_| CommandError::Arguments)?,
            replacement,
        )
        .map_err(|_| CommandError::Arguments)?,
    );
    let operation = match name.as_str() {
        "reset-records" => Operation::ResetRecords(request),
        "reset-catalogue" => Operation::ResetCatalogue(request),
        _ => return Err(CommandError::Arguments),
    };
    Ok(LocalCommand {
        cache: PathBuf::from(cache),
        receiver,
        origin,
        operation,
    })
}

fn identity(value: &str) -> Result<Id, CommandError> {
    Id::new(value).map_err(|_| CommandError::Identity)
}

fn page_count(value: &str) -> Result<usize, CommandError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CommandError::Arguments);
    }
    value.parse().map_err(|_| CommandError::Arguments)
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/entrypoint/arguments.rs"]
mod tests;
