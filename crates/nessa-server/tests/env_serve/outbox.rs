//! The outbox: what it stages, what it refuses, and its publish point.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    outbox: FileOutbox,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("work");
    fs::create_dir(&workspace).unwrap();
    let outbox = FileOutbox::open(
        root.path().join("outbox"),
        root.path().join("sockets"),
        &workspace,
    )
    .unwrap();
    let workspace = fs::canonicalize(workspace).unwrap();
    Fixture {
        _root: root,
        workspace,
        outbox,
    }
}

fn request(path: &Path) -> PublishRequest {
    PublishRequest {
        path: path.to_str().unwrap().to_owned(),
        media_type: None,
    }
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn a_file_in_the_workspace_is_staged_as_copied_and_hashed() {
    let fixture = fixture();
    let file = fixture.workspace.join("Nessa.dmg");
    let bytes = vec![7u8; 200_000];
    fs::write(&file, &bytes).unwrap();
    let staged = fixture.outbox.stage("lease-1", 1, &request(&file)).unwrap();
    assert_eq!(staged.name, "Nessa.dmg");
    assert_eq!(staged.media_type, "application/x-apple-diskimage");
    assert_eq!(staged.size, bytes.len() as u64);
    assert_eq!(staged.digest, hex(&bytes));
    // The copy is what the gateway reads: changing the source changes nothing.
    fs::write(&file, b"changed").unwrap();
    assert_eq!(fs::read(&staged.path).unwrap(), bytes);
    let mode = fs::metadata(&staged.path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    fixture.outbox.discard("lease-1", 1);
    assert!(!Path::new(&staged.path).exists());
}

#[test]
fn a_given_media_type_is_kept_and_an_ill_formed_one_refused() {
    let fixture = fixture();
    let file = fixture.workspace.join("shot");
    fs::write(&file, b"png bytes").unwrap();
    let mut asked = request(&file);
    asked.media_type = Some("image/png".into());
    assert_eq!(
        fixture.outbox.stage("lease", 1, &asked).unwrap().media_type,
        "image/png"
    );
    asked.media_type = Some("Image/PNG".into());
    assert_eq!(
        fixture.outbox.stage("lease", 2, &asked),
        Err(PublishRefusal::Invalid)
    );
    assert_eq!(
        fixture
            .outbox
            .stage("lease", 3, &request(&file))
            .unwrap()
            .media_type,
        "application/octet-stream"
    );
}

#[test]
fn nothing_outside_the_workspace_is_staged_even_through_a_link() {
    let fixture = fixture();
    let outside = fixture.workspace.parent().unwrap().join("secret");
    fs::write(&outside, b"not yours").unwrap();
    assert_eq!(
        fixture.outbox.stage("lease", 1, &request(&outside)),
        Err(PublishRefusal::OutsideWorkspace)
    );
    let link = fixture.workspace.join("innocent.png");
    symlink(&outside, &link).unwrap();
    assert_eq!(
        fixture.outbox.stage("lease", 2, &request(&link)),
        Err(PublishRefusal::OutsideWorkspace)
    );
    assert_eq!(
        fixture
            .outbox
            .stage("lease", 3, &request(Path::new("relative.png"))),
        Err(PublishRefusal::OutsideWorkspace)
    );
    assert_eq!(
        fixture
            .outbox
            .stage("lease", 4, &request(&fixture.workspace)),
        Err(PublishRefusal::OutsideWorkspace)
    );
}

#[test]
fn an_empty_or_missing_file_is_refused() {
    let fixture = fixture();
    let empty = fixture.workspace.join("empty");
    fs::write(&empty, b"").unwrap();
    assert_eq!(
        fixture.outbox.stage("lease", 1, &request(&empty)),
        Err(PublishRefusal::Size)
    );
    assert_eq!(
        fixture
            .outbox
            .stage("lease", 2, &request(&fixture.workspace.join("absent"))),
        Err(PublishRefusal::Unreadable)
    );
}

#[test]
fn a_lease_is_never_read_as_a_path() {
    let fixture = fixture();
    let file = fixture.workspace.join("a.txt");
    fs::write(&file, b"x").unwrap();
    let staged = fixture
        .outbox
        .stage("../../escape", 1, &request(&file))
        .unwrap();
    assert!(Path::new(&staged.path).starts_with(fixture._root.path().join("outbox")));
}

#[test]
fn closing_a_lease_lets_go_of_everything_it_staged() {
    let fixture = fixture();
    let file = fixture.workspace.join("a.txt");
    fs::write(&file, b"x").unwrap();
    let staged = fixture.outbox.stage("lease", 1, &request(&file)).unwrap();
    fixture.outbox.close("lease");
    assert!(!Path::new(&staged.path).exists());
}

#[tokio::test]
async fn the_publish_point_takes_one_request_and_gives_one_answer() {
    let fixture = fixture();
    let mut point = fixture.outbox.open("lease").unwrap();
    let address = point.address.clone();
    let publisher = tokio::spawn(async move {
        let mut stream = UnixStream::connect(&address).await.unwrap();
        stream
            .write_all(b"{\"path\":\"/work/a.png\"}\n")
            .await
            .unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).await.unwrap();
        answer
    });
    let call = point.calls.recv().await.unwrap();
    assert_eq!(call.request.path, "/work/a.png");
    call.answer
        .send(PublishAnswer::NotPublished {
            reason: PublishRefusal::Busy,
        })
        .unwrap();
    let answer = publisher.await.unwrap();
    assert_eq!(
        serde_json::from_str::<PublishAnswer>(&answer).unwrap(),
        PublishAnswer::NotPublished {
            reason: PublishRefusal::Busy
        }
    );
    fixture.outbox.close("lease");
}

#[tokio::test]
async fn an_unreadable_request_is_answered_invalid() {
    let fixture = fixture();
    let point = fixture.outbox.open("lease").unwrap();
    let mut stream = UnixStream::connect(&point.address).await.unwrap();
    stream.write_all(b"not json\n").await.unwrap();
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).await.unwrap();
    assert_eq!(
        serde_json::from_str::<PublishAnswer>(&answer).unwrap(),
        PublishAnswer::NotPublished {
            reason: PublishRefusal::Invalid
        }
    );
}

#[tokio::test]
async fn publishers_that_never_send_hold_no_more_than_their_bound() {
    let fixture = fixture();
    let mut point = fixture.outbox.open("lease").unwrap();
    let mut idle = Vec::new();
    for _ in 0..MAX_OPEN_PUBLISHERS {
        idle.push(UnixStream::connect(&point.address).await.unwrap());
    }
    let mut waiting = UnixStream::connect(&point.address).await.unwrap();
    waiting
        .write_all(b"{\"path\":\"/work/a.png\"}\n")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), point.calls.recv())
            .await
            .is_err(),
        "a publisher past the bound is not taken while the others are open"
    );
    drop(idle.pop());
    let call = point.calls.recv().await.unwrap();
    assert_eq!(call.request.path, "/work/a.png");
    drop(call);
    drop(idle);
    fixture.outbox.close("lease");
}
