//! The SFTP client against a scripted SFTP v3 server in memory: whole files,
//! resumes after a cut stream, short and reordered answers, and what it
//! refuses.
use super::{ByteSink, ReadFailure, SftpError, SftpSession, SinkWrite, MAX_PACKET, READ_CHUNK};
use scripted::{ScriptedSftp, SftpScript};
use std::{collections::HashMap, io, time::Duration};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};

/// A scripted SFTP v3 server, for these tests and the SSH adapter's.
pub(crate) mod scripted {
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
    };
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};

    /// How the server behaves, beyond serving its files.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct SftpScript {
        /// End the stream once this many file bytes were sent in all; the
        /// DATA that crosses it is cut short at it.
        pub(crate) cut_after: Option<u64>,
        /// Answer no READ with more than this many bytes.
        pub(crate) short_reads: Option<u32>,
        /// Answer READs two at a time, the later one first.
        pub(crate) reorder: bool,
        /// Answer EOF to any READ at or past this offset.
        pub(crate) eof_at: Option<u64>,
    }

    /// A running server: the client's ends of its stream.
    pub(crate) struct ScriptedSftp {
        pub(crate) reader: ReadHalf<DuplexStream>,
        pub(crate) writer: WriteHalf<DuplexStream>,
        /// File bytes sent so far, in all.
        pub(crate) sent: Arc<AtomicU64>,
    }

    impl ScriptedSftp {
        /// Serve `files` (by path) under `script`. A path not among them
        /// answers NO_SUCH_FILE.
        pub(crate) fn serve(files: HashMap<String, Vec<u8>>, script: SftpScript) -> Self {
            let (client, server) = duplex(1024 * 1024);
            let sent = Arc::new(AtomicU64::new(0));
            tokio::spawn(run(server, files, script, sent.clone()));
            let (reader, writer) = tokio::io::split(client);
            Self {
                reader,
                writer,
                sent,
            }
        }
    }

    struct Reply {
        bytes: Vec<u8>,
        /// File bytes it carries.
        data: u64,
    }

    fn packet(kind: u8, fields: &[&[u8]]) -> Reply {
        let body: Vec<u8> = fields.concat();
        let mut bytes = u32::try_from(body.len() + 1)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        bytes.push(kind);
        bytes.extend_from_slice(&body);
        Reply { bytes, data: 0 }
    }

    fn string(value: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(value.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(value);
        out
    }

    fn status(id: u32, code: u32) -> Reply {
        packet(
            101,
            &[
                &id.to_be_bytes(),
                &code.to_be_bytes(),
                &string(b""),
                &string(b""),
            ],
        )
    }

    struct Fields<'a>(&'a [u8]);

    impl Fields<'_> {
        fn u32(&mut self) -> u32 {
            let (head, rest) = self.0.split_at(4);
            self.0 = rest;
            u32::from_be_bytes(head.try_into().unwrap())
        }
        fn u64(&mut self) -> u64 {
            let (head, rest) = self.0.split_at(8);
            self.0 = rest;
            u64::from_be_bytes(head.try_into().unwrap())
        }
        fn bytes(&mut self) -> Vec<u8> {
            let len = self.u32() as usize;
            let (head, rest) = self.0.split_at(len);
            self.0 = rest;
            head.to_vec()
        }
    }

    async fn run(
        stream: DuplexStream,
        files: HashMap<String, Vec<u8>>,
        script: SftpScript,
        sent: Arc<AtomicU64>,
    ) {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let mut handles: HashMap<Vec<u8>, String> = HashMap::new();
        let mut held: Vec<Reply> = Vec::new();
        loop {
            let Ok(len) = reader.read_u32().await else {
                return;
            };
            let mut body = vec![0; len as usize];
            if reader.read_exact(&mut body).await.is_err() {
                return;
            }
            let kind = body[0];
            let mut fields = Fields(&body[1..]);
            // A READ waiting for its pair, unless it is the file's last.
            let mut wait_for_pair = false;
            let reply = match kind {
                1 => packet(2, &[&3u32.to_be_bytes(), &string(b"ext@x"), &string(b"1")]),
                3 => {
                    let id = fields.u32();
                    let path = String::from_utf8(fields.bytes()).unwrap();
                    if files.contains_key(&path) {
                        let handle = format!("h{}", handles.len()).into_bytes();
                        handles.insert(handle.clone(), path);
                        packet(102, &[&id.to_be_bytes(), &string(&handle)])
                    } else {
                        status(id, 2)
                    }
                }
                4 => {
                    let id = fields.u32();
                    handles.remove(&fields.bytes());
                    status(id, 0)
                }
                8 => {
                    let id = fields.u32();
                    let file = &files[&handles[&fields.bytes()]];
                    let size = file.len() as u64;
                    packet(
                        105,
                        &[&id.to_be_bytes(), &1u32.to_be_bytes(), &size.to_be_bytes()],
                    )
                }
                5 => {
                    let id = fields.u32();
                    let file = &files[&handles[&fields.bytes()]];
                    let offset = fields.u64();
                    let mut want = fields.u32();
                    if let Some(most) = script.short_reads {
                        want = want.min(most);
                    }
                    let end = script.eof_at.unwrap_or(u64::MAX).min(file.len() as u64);
                    wait_for_pair = offset + u64::from(want) < end;
                    if offset >= end {
                        status(id, 1)
                    } else {
                        let stop = end.min(offset + u64::from(want));
                        let data = &file[offset as usize..stop as usize];
                        let mut reply = packet(103, &[&id.to_be_bytes(), &string(data)]);
                        reply.data = data.len() as u64;
                        reply
                    }
                }
                other => panic!("the scripted server got packet type {other}"),
            };
            held.push(reply);
            if script.reorder && wait_for_pair && held.len() < 2 {
                continue;
            }
            while let Some(reply) = held.pop() {
                if !send(&mut writer, reply, &script, &sent).await {
                    return;
                }
            }
        }
    }

    /// Send `reply`, cutting the stream where the script says; false once
    /// it is cut.
    async fn send(
        writer: &mut WriteHalf<DuplexStream>,
        reply: Reply,
        script: &SftpScript,
        sent: &AtomicU64,
    ) -> bool {
        let before = sent.load(Ordering::SeqCst);
        if let Some(cut) = script.cut_after {
            if before + reply.data > cut {
                // The bytes up to the cut go out inside a DATA of the full
                // length, so the stream ends mid-packet.
                let keep = (cut - before) as usize;
                let header = reply.bytes.len() - reply.data as usize;
                let _ = writer.write_all(&reply.bytes[..header + keep]).await;
                sent.fetch_add(keep as u64, Ordering::SeqCst);
                let _ = writer.shutdown().await;
                return false;
            }
        }
        sent.fetch_add(reply.data, Ordering::SeqCst);
        writer.write_all(&reply.bytes).await.is_ok()
    }
}

/// Keeps every chunk it is given.
#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    chunks: usize,
}

impl ByteSink for Collected {
    fn write(&mut self, chunk: Vec<u8>) -> SinkWrite<'_> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.bytes.extend_from_slice(&chunk);
            self.chunks += 1;
            Ok(())
        })
    }
}

/// Refuses every chunk.
struct Refusing;

impl ByteSink for Refusing {
    fn write(&mut self, _chunk: Vec<u8>) -> SinkWrite<'_> {
        Box::pin(async { Err(io::Error::from(io::ErrorKind::StorageFull)) })
    }
}

/// A file of `len` bytes no two nearby chunks of which are alike.
fn file(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32).wrapping_mul(2_654_435_761).to_be_bytes()[0])
        .collect()
}

fn files(content: &[u8]) -> HashMap<String, Vec<u8>> {
    HashMap::from([("/outbox/l/1".to_owned(), content.to_vec())])
}

async fn session(
    server: ScriptedSftp,
) -> SftpSession<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>> {
    SftpSession::start(server.reader, server.writer)
        .await
        .expect("the session starts")
}

async fn read(
    content: &[u8],
    script: SftpScript,
    offset: u64,
) -> (Result<u64, ReadFailure>, Collected) {
    let server = ScriptedSftp::serve(files(content), script);
    let mut session = session(server).await;
    let mut sink = Collected::default();
    let result = session
        .read_file("/outbox/l/1", offset, content.len() as u64, &mut sink)
        .await;
    (result, sink)
}

#[tokio::test]
async fn whole_file_reaches_the_sink_in_order() {
    // More than one window's worth, and not a multiple of a chunk.
    let content = file(20 * READ_CHUNK as usize + 123);
    let (result, sink) = read(&content, SftpScript::default(), 0).await;
    assert_eq!(result, Ok(content.len() as u64));
    assert_eq!(sink.bytes, content);
}

#[tokio::test]
async fn a_resume_from_the_cut_offset_reproduces_the_exact_bytes() {
    let content = file(5 * READ_CHUNK as usize + 77);
    let cut = 2 * u64::from(READ_CHUNK) + 1000;
    let script = SftpScript {
        cut_after: Some(cut),
        ..SftpScript::default()
    };
    let (result, first) = read(&content, script, 0).await;
    let failure = result.expect_err("the cut stream fails the read");
    assert!(matches!(failure.error, SftpError::Unavailable(_)));
    // Only whole chunks reach the sink: the cut DATA never arrives whole.
    assert_eq!(failure.reached, 2 * u64::from(READ_CHUNK));
    assert_eq!(first.bytes, content[..failure.reached as usize]);

    let (resumed, rest) = read(&content, SftpScript::default(), failure.reached).await;
    assert_eq!(resumed, Ok(content.len() as u64));
    let mut whole = first.bytes;
    whole.extend_from_slice(&rest.bytes);
    assert_eq!(whole, content);
}

#[tokio::test]
async fn short_reads_ask_for_the_rest_and_skip_nothing() {
    let content = file(3 * READ_CHUNK as usize + 9);
    let script = SftpScript {
        short_reads: Some(1000),
        ..SftpScript::default()
    };
    let (result, sink) = read(&content, script, 0).await;
    assert_eq!(result, Ok(content.len() as u64));
    assert_eq!(sink.bytes, content);
    assert!(sink.chunks > 4, "each short DATA is its own chunk");
}

#[tokio::test]
async fn answers_out_of_order_still_reach_the_sink_in_order() {
    let content = file(40 * READ_CHUNK as usize + 5);
    let script = SftpScript {
        reorder: true,
        ..SftpScript::default()
    };
    let (result, sink) = read(&content, script, 0).await;
    assert_eq!(result, Ok(content.len() as u64));
    assert_eq!(sink.bytes, content);
}

#[tokio::test]
async fn a_missing_file_is_no_such_file() {
    let server = ScriptedSftp::serve(files(b"abc"), SftpScript::default());
    let mut session = session(server).await;
    let mut sink = Collected::default();
    let failure = session
        .read_file("/outbox/l/2", 0, 3, &mut sink)
        .await
        .expect_err("nothing is there");
    assert_eq!(
        failure,
        ReadFailure {
            error: SftpError::NoSuchFile,
            reached: 0
        }
    );
    // The session is still good for the next file.
    assert_eq!(
        session.read_file("/outbox/l/1", 0, 3, &mut sink).await,
        Ok(3)
    );
    assert_eq!(sink.bytes, b"abc");
}

#[tokio::test]
async fn a_size_other_than_published_is_size_changed_and_nothing_is_read() {
    let content = file(1000);
    let server = ScriptedSftp::serve(files(&content), SftpScript::default());
    let mut session = session(server).await;
    let mut sink = Collected::default();
    let failure = session
        .read_file("/outbox/l/1", 0, 999, &mut sink)
        .await
        .expect_err("the size differs");
    assert_eq!(failure.error, SftpError::SizeChanged);
    assert!(sink.bytes.is_empty());
    // The handle was closed and the stream left between packets.
    assert_eq!(
        session.read_file("/outbox/l/1", 0, 1000, &mut sink).await,
        Ok(1000)
    );
}

#[tokio::test]
async fn an_end_before_the_published_size_is_size_changed() {
    let content = file(3 * READ_CHUNK as usize);
    let script = SftpScript {
        eof_at: Some(u64::from(READ_CHUNK)),
        ..SftpScript::default()
    };
    let (result, sink) = read(&content, script, 0).await;
    let failure = result.expect_err("the file ends early");
    assert_eq!(failure.error, SftpError::SizeChanged);
    assert_eq!(failure.reached, sink.bytes.len() as u64);
    assert_eq!(sink.bytes, content[..sink.bytes.len()]);
}

#[tokio::test]
async fn a_sink_that_refuses_fails_the_read_on_this_side() {
    let content = file(100);
    let server = ScriptedSftp::serve(files(&content), SftpScript::default());
    let mut session = session(server).await;
    let failure = session
        .read_file("/outbox/l/1", 0, 100, &mut Refusing)
        .await
        .expect_err("the sink refuses");
    assert_eq!(
        failure,
        ReadFailure {
            error: SftpError::Sink(io::ErrorKind::StorageFull),
            reached: 0
        }
    );
}

#[tokio::test]
async fn an_oversize_packet_is_refused_from_its_length_alone() {
    let (client, mut server) = duplex(64 * 1024);
    let host = tokio::spawn(async move {
        let mut init = [0; 9];
        server.read_exact(&mut init).await.unwrap();
        // Only a length, one past the bound; no body ever follows.
        server
            .write_all(&(MAX_PACKET + 1).to_be_bytes())
            .await
            .unwrap();
        // Held open: a client waiting for the body would wait here.
        let mut rest = Vec::new();
        let _ = server.read_to_end(&mut rest).await;
    });
    let (reader, writer) = tokio::io::split(client);
    let started = tokio::time::timeout(Duration::from_secs(5), SftpSession::start(reader, writer))
        .await
        .expect("refused without waiting for the body");
    assert!(matches!(started, Err(SftpError::Protocol(_))));
    drop(started);
    host.await.unwrap();
}

#[tokio::test]
async fn a_packet_at_the_bound_is_read() {
    // A VERSION padded to exactly MAX_PACKET with an extension.
    let (client, mut server) = duplex(512 * 1024);
    let host = tokio::spawn(async move {
        let mut init = [0; 9];
        server.read_exact(&mut init).await.unwrap();
        let pad = MAX_PACKET as usize - 1 - 4 - 4 - 4 - 1;
        let mut packet = MAX_PACKET.to_be_bytes().to_vec();
        packet.push(2);
        packet.extend_from_slice(&3u32.to_be_bytes());
        packet.extend_from_slice(&1u32.to_be_bytes());
        packet.push(b'x');
        packet.extend_from_slice(&u32::try_from(pad).unwrap().to_be_bytes());
        packet.extend(std::iter::repeat_n(0, pad));
        server.write_all(&packet).await.unwrap();
        server
    });
    let (reader, writer) = tokio::io::split(client);
    assert!(SftpSession::start(reader, writer).await.is_ok());
    drop(host.await.unwrap());
}

#[tokio::test]
async fn an_older_version_is_refused() {
    let (client, mut server) = duplex(1024);
    let host = tokio::spawn(async move {
        let mut init = [0; 9];
        server.read_exact(&mut init).await.unwrap();
        server
            .write_all(&[0, 0, 0, 5, 2, 0, 0, 0, 2])
            .await
            .unwrap();
        server
    });
    let (reader, writer) = tokio::io::split(client);
    assert!(matches!(
        SftpSession::start(reader, writer).await,
        Err(SftpError::Protocol(_))
    ));
    drop(host.await.unwrap());
}

#[tokio::test]
async fn a_stream_that_ends_at_start_is_unavailable() {
    let (client, server) = duplex(1024);
    drop(server);
    let (reader, writer) = tokio::io::split(client);
    assert!(matches!(
        SftpSession::start(reader, writer).await,
        Err(SftpError::Unavailable(_))
    ));
}
