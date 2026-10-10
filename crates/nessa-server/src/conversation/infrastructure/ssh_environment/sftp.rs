//! The least of SFTP version 3 the gateway needs: pulling one published file
//! off a host, over the `sftp` subsystem of the lease's own `ssh` connection.
//!
//! ```text
//! start: INIT v3 ──▶ VERSION ≥ 3 (its extensions read past) ──▶ a session
//! read_file(path, offset, size, sink):
//!   OPEN(path, READ) ──▶ HANDLE     NO_SUCH_FILE / PERMISSION_DENIED ──▶ NoSuchFile
//!   FSTAT ──▶ ATTRS: size == size?  no ──▶ SizeChanged
//!   READ 32 KiB, at most 16 asked ──▶ DATA, in any order ──▶ held by offset
//!        ──▶ the sink, strictly in file order; a short DATA asks for the rest
//!   EOF before `size` ──▶ SizeChanged
//!   CLOSE ──▶ STATUS                (best effort once the read failed)
//!   ──▶ the offset reached, whether it finished or not
//! ```
//!
//! Arrows are packets and what follows from them. Every packet's length is
//! checked against [`MAX_PACKET`] before anything is allocated for it, and a
//! read never holds more than [`READ_WINDOW`] bytes of the file in memory:
//! nothing is asked past what has reached the sink by more than that. A
//! stream that ends or fails is [`SftpError::Unavailable`], and the offset
//! reached is where a new session resumes. Waits are bounded by the caller,
//! which owns the connection.
use std::{collections::BTreeMap, fmt, future::Future, io, pin::Pin};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The protocol version spoken.
const VERSION: u32 = 3;
/// Bytes asked for in one READ.
pub(crate) const READ_CHUNK: u32 = 32 * 1024;
/// Most READs asked and not yet answered.
pub(crate) const MAX_OUTSTANDING: usize = 16;
/// Most bytes asked past what has reached the sink: what may be held here.
const READ_WINDOW: u64 = READ_CHUNK as u64 * MAX_OUTSTANDING as u64;
/// The longest packet accepted, its type byte included: the largest DATA
/// this client could provoke, with room for its header.
pub(crate) const MAX_PACKET: u32 = 256 * 1024 + 1024;
/// The longest file handle a server may hand back (the draft's own bound).
const MAX_HANDLE: usize = 256;

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_READ: u8 = 5;
const FXP_FSTAT: u8 = 8;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_DATA: u8 = 103;
const FXP_ATTRS: u8 = 105;

const FXF_READ: u32 = 0x0000_0001;
const ATTR_SIZE: u32 = 0x0000_0001;

const FX_OK: u32 = 0;
const FX_EOF: u32 = 1;
const FX_NO_SUCH_FILE: u32 = 2;
const FX_PERMISSION_DENIED: u32 = 3;

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SftpError {
    /// The stream ended or failed: the subsystem is absent, or the
    /// connection went. A new session may resume from the offset reached.
    Unavailable(io::ErrorKind),
    /// The host has no such file, or will not open it for us.
    NoSuchFile,
    /// The file is not the size it was published at: `FSTAT` disagreed,
    /// or it ended before that size.
    SizeChanged,
    /// The sink refused bytes: a failure on this side, not the host's.
    Sink(io::ErrorKind),
    /// The host answered something this client cannot accept.
    Protocol(String),
}

impl fmt::Display for SftpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(kind) => write!(f, "the sftp stream is unavailable: {kind}"),
            Self::NoSuchFile => f.write_str("the host has no such file to read"),
            Self::SizeChanged => f.write_str("the file is not the size it was published at"),
            Self::Sink(kind) => write!(f, "the file's bytes could not be kept: {kind}"),
            Self::Protocol(why) => write!(f, "the host broke the sftp protocol: {why}"),
        }
    }
}

impl std::error::Error for SftpError {}

impl From<io::Error> for SftpError {
    fn from(error: io::Error) -> Self {
        Self::Unavailable(error.kind())
    }
}

fn protocol(why: impl Into<String>) -> SftpError {
    SftpError::Protocol(why.into())
}

/// A read that stopped: why, and how far into the file its bytes reached
/// the sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadFailure {
    pub(crate) error: SftpError,
    /// Every byte before this offset was given to the sink, in order.
    pub(crate) reached: u64,
}

/// The future one [`ByteSink::write`] answers with.
pub(crate) type SinkWrite<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;

/// Where a file's bytes go, in file order.
pub(crate) trait ByteSink: Send {
    /// Keep `chunk`, the bytes that follow every chunk given before it.
    fn write(&mut self, chunk: Vec<u8>) -> SinkWrite<'_>;
}

/// One SFTP session over a byte stream: an `ssh -s sftp` child's stdout and
/// stdin, or an in-memory pipe in tests.
pub(crate) struct SftpSession<R, W> {
    reader: R,
    writer: W,
    next_id: u32,
    /// Set once a failure left the stream between packets or unanswered:
    /// nothing more is asked on it.
    broken: bool,
}

/// A READ asked and not yet answered.
#[derive(Clone, Copy)]
struct Asked {
    offset: u64,
    len: u32,
}

impl<R, W> SftpSession<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    /// Say INIT and wait for the host's VERSION.
    ///
    /// # Errors
    /// [`SftpError::Unavailable`] when the stream ends first;
    /// [`SftpError::Protocol`] when the answer is not a VERSION of at least 3.
    pub(crate) async fn start(reader: R, writer: W) -> Result<Self, SftpError> {
        let mut session = Self {
            reader,
            writer,
            next_id: 0,
            broken: false,
        };
        let mut init = Packet::new(FXP_INIT);
        init.u32(VERSION);
        session.send(init).await?;
        let (kind, body) = session.receive().await?;
        if kind != FXP_VERSION {
            return Err(protocol(format!(
                "expected VERSION, got packet type {kind}"
            )));
        }
        // What follows the version is extension pairs, already bounded by
        // the packet's length; none is used.
        let version = Fields::new(&body).u32()?;
        if version < VERSION {
            return Err(protocol(format!("version {version} is older than 3")));
        }
        Ok(session)
    }

    /// Read `path` from `offset` to `expected_size` into `sink`, in order.
    ///
    /// Returns `expected_size`, the offset reached. The file is closed
    /// after, and closed best effort when the read fails.
    ///
    /// # Errors
    /// A [`ReadFailure`] naming the error and the offset its bytes reached.
    /// After [`SftpError::Unavailable`] a new session may resume from it.
    pub(crate) async fn read_file(
        &mut self,
        path: &str,
        offset: u64,
        expected_size: u64,
        sink: &mut dyn ByteSink,
    ) -> Result<u64, ReadFailure> {
        let fail = |error| ReadFailure {
            error,
            reached: offset,
        };
        if self.broken {
            return Err(fail(SftpError::Unavailable(io::ErrorKind::BrokenPipe)));
        }
        if offset > expected_size {
            return Err(fail(protocol("the resume offset is past the file's size")));
        }
        let handle = self
            .open(path)
            .await
            .map_err(|error| fail(self.note(error)))?;
        let mut reached = offset;
        let read = self
            .read_open(&handle, expected_size, &mut reached, sink)
            .await;
        let closed = self.close(&handle, read.is_ok()).await;
        match (read, closed) {
            (Err(error), _) | (Ok(()), Err(error)) => Err(ReadFailure {
                error: self.note(error),
                reached,
            }),
            (Ok(()), Ok(())) => Ok(reached),
        }
    }

    /// Mark the stream unusable after an error that leaves it unsettled.
    fn note(&mut self, error: SftpError) -> SftpError {
        if matches!(
            error,
            SftpError::Unavailable(_) | SftpError::Protocol(_) | SftpError::Sink(_)
        ) {
            self.broken = true;
        }
        error
    }

    async fn open(&mut self, path: &str) -> Result<Vec<u8>, SftpError> {
        let id = self.id();
        let mut open = Packet::new(FXP_OPEN);
        open.u32(id);
        open.bytes(path.as_bytes());
        open.u32(FXF_READ);
        open.u32(0); // no attributes
        self.send(open).await?;
        let (kind, body) = self.receive().await?;
        let mut fields = Fields::new(&body);
        answers(id, fields.u32()?)?;
        match kind {
            FXP_HANDLE => {
                let handle = fields.bytes()?;
                if handle.is_empty() || handle.len() > MAX_HANDLE {
                    return Err(protocol("a file handle out of bounds"));
                }
                Ok(handle.to_vec())
            }
            FXP_STATUS => match fields.u32()? {
                FX_NO_SUCH_FILE | FX_PERMISSION_DENIED => Err(SftpError::NoSuchFile),
                code => Err(protocol(format!("OPEN answered status {code}"))),
            },
            other => Err(protocol(format!("OPEN answered packet type {other}"))),
        }
    }

    async fn read_open(
        &mut self,
        handle: &[u8],
        expected_size: u64,
        reached: &mut u64,
        sink: &mut dyn ByteSink,
    ) -> Result<(), SftpError> {
        if self.size(handle).await? != expected_size {
            return Err(SftpError::SizeChanged);
        }
        let mut asked: BTreeMap<u32, Asked> = BTreeMap::new();
        // Bytes answered ahead of `reached`, by offset; never past the window.
        let mut held: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        let mut frontier = *reached;
        loop {
            while asked.len() < MAX_OUTSTANDING
                && frontier < expected_size
                && frontier - *reached < READ_WINDOW
            {
                let room = (expected_size - frontier)
                    .min(READ_WINDOW - (frontier - *reached))
                    .min(u64::from(READ_CHUNK));
                let len = u32::try_from(room).unwrap_or(READ_CHUNK);
                self.ask(handle, frontier, len, &mut asked).await?;
                frontier += u64::from(len);
            }
            if asked.is_empty() {
                return Ok(());
            }
            let (kind, body) = self.receive().await?;
            let mut fields = Fields::new(&body);
            let id = fields.u32()?;
            let request = asked
                .remove(&id)
                .ok_or_else(|| protocol(format!("an answer to unknown request {id}")))?;
            match kind {
                FXP_DATA => {
                    let data = fields.bytes()?;
                    let got = u32::try_from(data.len()).unwrap_or(u32::MAX);
                    if got == 0 || got > request.len {
                        return Err(protocol(format!(
                            "DATA of {got} bytes for a READ of {}",
                            request.len
                        )));
                    }
                    if got < request.len {
                        // A short read: the rest is asked again, never skipped.
                        let rest = request.offset + u64::from(got);
                        self.ask(handle, rest, request.len - got, &mut asked)
                            .await?;
                    }
                    held.insert(request.offset, data.to_vec());
                }
                FXP_STATUS => {
                    return Err(match fields.u32()? {
                        FX_EOF => SftpError::SizeChanged,
                        code => protocol(format!("READ answered status {code}")),
                    })
                }
                other => return Err(protocol(format!("READ answered packet type {other}"))),
            }
            while let Some(chunk) = held.remove(&*reached) {
                let len = chunk.len() as u64;
                sink.write(chunk)
                    .await
                    .map_err(|error| SftpError::Sink(error.kind()))?;
                *reached += len;
            }
        }
    }

    async fn size(&mut self, handle: &[u8]) -> Result<u64, SftpError> {
        let id = self.id();
        let mut fstat = Packet::new(FXP_FSTAT);
        fstat.u32(id);
        fstat.bytes(handle);
        self.send(fstat).await?;
        let (kind, body) = self.receive().await?;
        let mut fields = Fields::new(&body);
        answers(id, fields.u32()?)?;
        match kind {
            FXP_ATTRS => {
                if fields.u32()? & ATTR_SIZE == 0 {
                    return Err(protocol("FSTAT gave no size"));
                }
                fields.u64()
            }
            FXP_STATUS => Err(protocol(format!("FSTAT answered status {}", fields.u32()?))),
            other => Err(protocol(format!("FSTAT answered packet type {other}"))),
        }
    }

    async fn ask(
        &mut self,
        handle: &[u8],
        offset: u64,
        len: u32,
        asked: &mut BTreeMap<u32, Asked>,
    ) -> Result<(), SftpError> {
        let id = self.id();
        let mut read = Packet::new(FXP_READ);
        read.u32(id);
        read.bytes(handle);
        read.u64(offset);
        read.u32(len);
        self.send(read).await?;
        asked.insert(id, Asked { offset, len });
        Ok(())
    }

    /// Close `handle`. After a clean read its answer must be OK; after a
    /// failed one it is best effort: skipped when the stream is gone or
    /// unreadable, and any READs still unanswered are read past.
    async fn close(&mut self, handle: &[u8], clean: bool) -> Result<(), SftpError> {
        if self.broken {
            return Ok(());
        }
        let id = self.id();
        let mut close = Packet::new(FXP_CLOSE);
        close.u32(id);
        close.bytes(handle);
        let answer = async {
            self.send(close).await?;
            // Each READ asked is answered once, and at most MAX_OUTSTANDING
            // are; anything more is the host misbehaving.
            for _ in 0..=MAX_OUTSTANDING {
                let (kind, body) = self.receive().await?;
                let mut fields = Fields::new(&body);
                if fields.u32()? != id {
                    continue;
                }
                return match (kind, fields.u32()?) {
                    (FXP_STATUS, FX_OK) => Ok(()),
                    (FXP_STATUS, code) => Err(protocol(format!("CLOSE answered status {code}"))),
                    (other, _) => Err(protocol(format!("CLOSE answered packet type {other}"))),
                };
            }
            Err(protocol("no answer to CLOSE"))
        }
        .await;
        match answer {
            Err(error) if clean => Err(error),
            Err(error) => {
                self.note(error);
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    async fn send(&mut self, packet: Packet) -> Result<(), SftpError> {
        self.writer.write_all(&packet.finish()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// The next packet: its type and the rest of its body. Its length is
    /// checked before anything is allocated for it.
    async fn receive(&mut self) -> Result<(u8, Vec<u8>), SftpError> {
        let len = self.reader.read_u32().await?;
        if len == 0 || len > MAX_PACKET {
            return Err(protocol(format!("a packet of {len} bytes")));
        }
        let kind = self.reader.read_u8().await?;
        let mut body = vec![0; len as usize - 1];
        self.reader.read_exact(&mut body).await?;
        Ok((kind, body))
    }
}

/// Refuse an answer to a request other than the one waited on.
fn answers(id: u32, got: u32) -> Result<(), SftpError> {
    if got == id {
        Ok(())
    } else {
        Err(protocol(format!("answer to request {got}, not {id}")))
    }
}

/// A packet being written: its length is set when it is finished.
struct Packet(Vec<u8>);

impl Packet {
    fn new(kind: u8) -> Self {
        Self(vec![0, 0, 0, 0, kind])
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn bytes(&mut self, value: &[u8]) {
        self.u32(u32::try_from(value.len()).unwrap_or(u32::MAX));
        self.0.extend_from_slice(value);
    }

    fn finish(mut self) -> Vec<u8> {
        let len = u32::try_from(self.0.len() - 4).unwrap_or(u32::MAX);
        self.0[..4].copy_from_slice(&len.to_be_bytes());
        self.0
    }
}

/// The fields of a packet's body, read in order; reading past its end is a
/// protocol error.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn new(body: &'a [u8]) -> Self {
        Self(body)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], SftpError> {
        if self.0.len() < n {
            return Err(protocol("a packet ends inside a field"));
        }
        let (field, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(field)
    }

    fn u32(&mut self) -> Result<u32, SftpError> {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, SftpError> {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(bytes))
    }

    fn bytes(&mut self) -> Result<&'a [u8], SftpError> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/sftp.rs"]
pub(super) mod tests;
