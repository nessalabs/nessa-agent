use crate::read_only_sync::application::{CacheError, GatewayError};
use crate::read_only_sync::domain::{CacheReset, HeldFile};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use nessa_sync::replication::catalogue::EntryKey;
use nessa_sync::replication::domain::{Id, Scope};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;
use std::path::PathBuf;

pub(crate) const HELP: &str = "read_only_sync pair PROFILE [peer] (code on stdin)\nread_only_sync status PROFILE\nread_only_sync comment PROFILE CONVERSATION TEXT\nread_only_sync drive PROFILE CONVERSATION TEXT\nread_only_sync sync-records PROFILE CONVERSATION PAGES\nread_only_sync check-records PROFILE CONVERSATION\nread_only_sync follow PROFILE CONVERSATION PAGES MAX_PASSES\nread_only_sync sync-catalogue PROFILE PAGES\nread_only_sync fetch PROFILE CONVERSATION DIGEST MIMETYPE SIZE OUT\nread_only_sync list CACHE RECEIVER ORIGIN CATALOGUE [AFTER_CREATION AFTER_ID]\nread_only_sync show CACHE RECEIVER ORIGIN CONVERSATION\nread_only_sync reset-records|reset-catalogue CACHE RECEIVER ORIGIN STREAM OPERATION CALLER GENERATION OLD_INCARNATION OLD_SCHEMA OLD_EPOCH NEW_INCARNATION NEW_SCHEMA NEW_EPOCH";

/// Who `pair` enrolls as. `peer` is a gateway principal (`ConsentClass::PeerRead`);
/// anything else is one of the owner's devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairClass {
    Device,
    Peer,
}

pub(crate) enum Command {
    Local(LocalCommand),
    /// Enroll this device with the code read from standard input.
    Pair {
        profile: PathBuf,
        class: PairClass,
    },
    /// Leave a note on a conversation this enrollment may comment on.
    Comment {
        profile: PathBuf,
        conversation: ConversationId,
        text: String,
    },
    /// Send one text turn on a conversation this enrollment may drive.
    Drive {
        profile: PathBuf,
        conversation: ConversationId,
        text: String,
    },
    /// Read this device's enrollment status over the pinned gateway key; a
    /// Terminal status purges the profile's cache before the record goes.
    Status {
        profile: PathBuf,
    },
    /// Online commands read into the cache their profile names.
    Records {
        profile: PathBuf,
        conversation: ConversationId,
        pages: usize,
    },
    Catalogue {
        profile: PathBuf,
        pages: usize,
    },
    /// Follow one conversation through `conversation.subscribe` on one
    /// connection, copying its records into the cache for at most
    /// `max_passes` passes, the recheck pass included.
    Follow {
        profile: PathBuf,
        conversation: ConversationId,
        pages: usize,
        max_passes: NonZeroUsize,
    },
    /// Fetch one file the conversation holds into `out`, assembled in a part
    /// beside it named for the file's digest, and kept under `out` only once
    /// its size and digest are verified and nothing is at `out`.
    Fetch {
        profile: PathBuf,
        file: HeldFile,
        out: PathBuf,
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
    /// A fetch stopped on this device's files: its output or part.
    LocalFile,
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
            Self::LocalFile => out.write_str("local file refused; see JSON outcome"),
        }
    }
}

pub(crate) fn parse(args: &[String]) -> Result<Command, CommandError> {
    if let Some(command) = parse_granted(args)? {
        return Ok(command);
    }
    match args {
        [name, profile, class] if name == "pair" && class == "peer" => {
            return Ok(Command::Pair {
                profile: profile.into(),
                class: PairClass::Peer,
            })
        }
        [name, profile] if name == "pair" => {
            return Ok(Command::Pair {
                profile: profile.into(),
                class: PairClass::Device,
            })
        }
        [name, profile] if name == "status" => {
            return Ok(Command::Status {
                profile: profile.into(),
            })
        }
        [name, profile, conversation, pages] if name == "sync-records" => {
            return Ok(Command::Records {
                profile: profile.into(),
                conversation: ConversationId::new(conversation)
                    .map_err(|_| CommandError::Identity)?,
                pages: count(pages)?,
            })
        }
        [name, profile, conversation] if name == "check-records" => {
            return Ok(Command::Records {
                profile: profile.into(),
                conversation: ConversationId::new(conversation)
                    .map_err(|_| CommandError::Identity)?,
                pages: 0,
            })
        }
        [name, profile, conversation, pages, passes] if name == "follow" => {
            return Ok(Command::Follow {
                profile: profile.into(),
                conversation: ConversationId::new(conversation)
                    .map_err(|_| CommandError::Identity)?,
                pages: count(pages)?,
                max_passes: NonZeroUsize::new(count(passes)?).ok_or(CommandError::Arguments)?,
            })
        }
        [name, profile, conversation, digest, mime_type, size, out] if name == "fetch" => {
            return Ok(Command::Fetch {
                profile: profile.into(),
                file: HeldFile::new(
                    ConversationId::new(conversation).map_err(|_| CommandError::Identity)?,
                    Sha256Digest::parse(digest).map_err(|_| CommandError::Arguments)?,
                    MediaType::parse(mime_type).map_err(|_| CommandError::Arguments)?,
                    count(size)?,
                )
                .map_err(|_| CommandError::Arguments)?,
                out: out.into(),
            })
        }
        [name, profile, pages] if name == "sync-catalogue" => {
            return Ok(Command::Catalogue {
                profile: profile.into(),
                pages: count(pages)?,
            })
        }
        _ => {}
    }
    parse_local(args).map(Command::Local)
}

/// `comment` and `drive` name a conversation and the text, which may be several
/// words. A missing or blank text, or one past 8 KiB, is an argument error.
fn parse_granted(args: &[String]) -> Result<Option<Command>, CommandError> {
    let [name, profile, conversation, text @ ..] = args else {
        return Ok(None);
    };
    if name != "comment" && name != "drive" {
        return Ok(None);
    }
    if text.is_empty() {
        return Err(CommandError::Arguments);
    }
    let text = text.join(" ");
    if text.len() > 8 * 1024 || text.trim().is_empty() {
        return Err(CommandError::Arguments);
    }
    let conversation = ConversationId::new(conversation).map_err(|_| CommandError::Identity)?;
    let profile = PathBuf::from(profile);
    Ok(Some(if name == "drive" {
        Command::Drive {
            profile,
            conversation,
            text,
        }
    } else {
        Command::Comment {
            profile,
            conversation,
            text,
        }
    }))
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

/// Plain decimal digits only: no sign, space or other spelling `parse` takes.
fn count<T: std::str::FromStr>(value: &str) -> Result<T, CommandError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CommandError::Arguments);
    }
    value.parse().map_err(|_| CommandError::Arguments)
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/entrypoint/arguments.rs"]
mod tests;
