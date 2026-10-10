//! `nessa artifact publish PATH`: attach a file in the workspace to the
//! conversation whose agent runs this command on a host serving
//! `nessa env serve` (issue #701).
//!
//! ```text
//! NESSA_ARTIFACTS (set by env serve for the lease) ──▶ connect
//!   ──▶ one request line { path (absolute), mediaType? }
//!   ◀── one answer line: held | alreadyHeld | refused | notPublished
//!   ──▶ printed as JSON on stdout; success only for held and alreadyHeld
//! ```
//!
//! Arrows are steps, in order. The answer comes once the gateway has read
//! the file over the artifact channel and verified it by its digest, or
//! refused it; this command decides nothing itself.
use crate::{
    core::RunError,
    env_serve::application::{PublishAnswer, PublishRequest, PUBLISH_POINT_VARIABLE},
};
use std::{io::Write, path::Path};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

/// Longest answer line read.
const MAX_ANSWER_BYTES: u64 = 4 * 1024;

/// Publish `path`, and say whether the conversation holds it now.
///
/// # Errors
/// No publish point to ask, or it could not be reached or answered nothing.
pub(super) async fn execute(path: &Path, media_type: Option<String>) -> Result<bool, RunError> {
    let point = std::env::var(PUBLISH_POINT_VARIABLE).map_err(|_| {
        RunError::Agent(format!(
            "{PUBLISH_POINT_VARIABLE} is not set: artifact publish runs only under an agent a gateway leased to this host"
        ))
    })?;
    let absolute = std::path::absolute(path)
        .map_err(|error| RunError::Agent(format!("{}: {error}", path.display())))?;
    let path = absolute
        .to_str()
        .ok_or_else(|| RunError::Agent("the path is not UTF-8".into()))?
        .to_owned();
    let request = PublishRequest { path, media_type };
    let mut line =
        serde_json::to_vec(&request).map_err(|error| RunError::Agent(error.to_string()))?;
    line.push(b'\n');
    let stream = UnixStream::connect(&point).await.map_err(|error| {
        RunError::Agent(format!("the publish point could not be reached: {error}"))
    })?;
    let (read, mut write) = stream.into_split();
    write.write_all(&line).await?;
    write.shutdown().await?;
    let mut answer = String::new();
    BufReader::new(read.take(MAX_ANSWER_BYTES))
        .read_line(&mut answer)
        .await?;
    let answer: PublishAnswer = serde_json::from_str(&answer)
        .map_err(|_| RunError::Agent("the publish point answered nothing readable".into()))?;
    let mut printed =
        serde_json::to_vec(&answer).map_err(|error| RunError::Agent(error.to_string()))?;
    printed.push(b'\n');
    std::io::stdout().write_all(&printed)?;
    Ok(matches!(
        answer,
        PublishAnswer::Held { .. } | PublishAnswer::AlreadyHeld { .. }
    ))
}
