use super::*;
use std::io::Cursor;

fn every_frame() -> Vec<RelayFrame> {
    vec![
        RelayFrame::Serve { key: [1; 32] },
        RelayFrame::Connect { key: [2; 32] },
        RelayFrame::Accept {
            key: [3; 32],
            token: [4; 16],
        },
        RelayFrame::Challenge { nonce: [5; 32] },
        RelayFrame::Proof { signature: [6; 64] },
        RelayFrame::Registered,
        RelayFrame::Incoming { token: [7; 16] },
        RelayFrame::Ping,
        RelayFrame::Pong,
        RelayFrame::Joined,
        RelayFrame::Offline,
        RelayFrame::NoAnswer,
    ]
}

#[test]
fn every_frame_reads_back_as_itself_and_fits_the_bound() {
    for frame in every_frame() {
        let bytes = frame.encode();
        assert!(bytes.len() <= MAX_FRAME, "{frame:?}");
        assert_eq!(read_frame(&mut Cursor::new(&bytes)).unwrap(), frame);
    }
}

#[test]
fn reading_a_frame_leaves_the_bytes_after_it_on_the_stream() {
    let mut bytes = RelayFrame::Joined.encode();
    bytes.extend_from_slice(b"\x16\x03\x01tls");
    let mut stream = Cursor::new(bytes);
    assert_eq!(read_frame(&mut stream).unwrap(), RelayFrame::Joined);
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"\x16\x03\x01tls");
}

#[test]
fn another_version_an_unknown_type_or_a_wrong_length_is_refused_before_its_body() {
    assert_eq!(parse_header([2, 6, 0]), Err(RelayFrameError::Version(2)));
    assert_eq!(parse_header([VERSION, 0, 0]), Err(RelayFrameError::Type(0)));
    assert_eq!(
        parse_header([VERSION, 13, 0]),
        Err(RelayFrameError::Type(13))
    );
    assert_eq!(
        parse_header([VERSION, 1, 255]),
        Err(RelayFrameError::Length {
            kind: 1,
            length: 255
        })
    );
    assert_eq!(
        parse_header([VERSION, 6, 1]),
        Err(RelayFrameError::Length { kind: 6, length: 1 })
    );
    // The stream is read no further than the header.
    let mut stream = Cursor::new(vec![VERSION, 1, 255, 0, 0]);
    let error = read_frame(&mut stream).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(stream.position(), HEADER as u64);
}

#[test]
fn a_body_shorter_than_its_header_is_refused() {
    let header = parse_header([VERSION, 1, 32]).unwrap();
    assert!(parse_body(header, &[0; 31]).is_err());
    let mut stream = Cursor::new(vec![VERSION, 1, 32, 0]);
    assert_eq!(
        read_frame(&mut stream).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn debug_names_a_key_short_and_shows_no_secret() {
    let shown = format!(
        "{:?} {:?} {:?}",
        RelayFrame::Serve { key: [0xab; 32] },
        RelayFrame::Proof {
            signature: [0xcd; 64]
        },
        RelayFrame::Incoming { token: [0xef; 16] }
    );
    assert_eq!(shown, "Serve(abababab) Proof Incoming");
}
