//! Byte channels for bounded artifact transfer alongside an environment link.
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

/// One artifact channel: an sftp session's two directions, and whatever
/// keeps it open.
pub(crate) struct ArtifactChannel {
    /// What the host's sftp server writes.
    pub(crate) from_host: Box<dyn AsyncRead + Send + Unpin>,
    /// What it reads.
    pub(crate) to_host: Box<dyn AsyncWrite + Send + Unpin>,
    /// Dropping it ends the channel, never the connection it rides on.
    pub(crate) keep: Box<dyn Send>,
}

/// Opens artifact channels beside one connection's lease frames: sftp on
/// the same SSH connection, so nothing more is authenticated or reached.
pub(crate) trait ArtifactChannels: Send + Sync {
    /// Open one. Answers at once; whether it is served is learned from what
    /// arrives on it.
    ///
    /// # Errors
    /// Nothing could be started at all.
    fn open(&self) -> io::Result<ArtifactChannel>;
}
