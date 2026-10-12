use super::{enroll, enroll_once, EnrolledOnce, EnrollingPairing, EnrollmentCode, NotEnrolled};
use crate::env_serve::application::{Answer, EnvironmentPairing, Saved};
use std::sync::Mutex;

const HERE: &str = "gw.example:47650";

/// A pairing that answers from its fields and keeps, in order, what was
/// done to it.
struct Scripted {
    saved: Result<Saved<()>, String>,
    recorded: Result<Option<String>, String>,
    record: Result<(), String>,
    send: Result<(), String>,
    done: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(saved: Saved<()>, recorded: Option<&str>) -> Self {
        Self {
            saved: Ok(saved),
            recorded: Ok(recorded.map(str::to_owned)),
            record: Ok(()),
            send: Ok(()),
            done: Mutex::new(Vec::new()),
        }
    }

    fn done(&self) -> Vec<String> {
        self.done.lock().unwrap().clone()
    }
}

impl EnvironmentPairing for Scripted {
    type Approval = ();

    async fn saved(&self) -> Result<Saved<()>, String> {
        match &self.saved {
            Ok(Saved::Approved(())) => Ok(Saved::Approved(())),
            Ok(Saved::Pending) => Ok(Saved::Pending),
            Ok(Saved::Nothing) => Ok(Saved::Nothing),
            Err(reason) => Err(reason.clone()),
        }
    }

    async fn ask(&self) -> Answer {
        unreachable!("enrolling never asks about approval")
    }
}

impl EnrollingPairing for Scripted {
    type Code = u32;

    fn address(&self) -> &str {
        HERE
    }

    async fn recorded_address(&self) -> Result<Option<String>, String> {
        self.recorded.clone()
    }

    async fn record_address(&self) -> Result<(), String> {
        self.done.lock().unwrap().push("record".into());
        self.record.clone()
    }

    async fn send_code(&self, code: u32) -> Result<(), String> {
        self.done.lock().unwrap().push(format!("send {code}"));
        self.send.clone()
    }
}

/// A code source that answers `code`, counting each read.
struct Code {
    code: Result<u32, String>,
    reads: Mutex<u32>,
}

impl Code {
    fn new(code: Result<u32, String>) -> Self {
        Self {
            code,
            reads: Mutex::new(0),
        }
    }

    fn reads(&self) -> u32 {
        *self.reads.lock().unwrap()
    }
}

impl EnrollmentCode<u32> for Code {
    fn read(&self) -> Result<u32, String> {
        *self.reads.lock().unwrap() += 1;
        self.code.clone()
    }
}

#[tokio::test]
async fn nothing_paired_records_the_address_then_sends_the_code() {
    let pairing = Scripted::new(Saved::Nothing, None);
    let code = Code::new(Ok(7));
    assert_eq!(enroll_once(&pairing, &code).await, Ok(EnrolledOnce::Now));
    assert_eq!(pairing.done(), ["record", "send 7"]);
    assert_eq!(code.reads(), 1);
}

#[tokio::test]
async fn a_pairing_with_this_gateway_reads_and_sends_nothing() {
    for saved in [Saved::Pending, Saved::Approved(())] {
        let pairing = Scripted::new(saved, Some(HERE));
        let code = Code::new(Err("never read".into()));
        assert_eq!(enroll_once(&pairing, &code).await, Ok(EnrolledOnce::Before));
        assert!(pairing.done().is_empty());
        assert_eq!(code.reads(), 0);
    }
}

#[tokio::test]
async fn a_pairing_with_another_gateway_or_none_recorded_is_refused_untouched() {
    for (recorded, refused) in [
        (
            Some("other.example:47650"),
            NotEnrolled::PairedElsewhere(Some("other.example:47650".into())),
        ),
        (None, NotEnrolled::PairedElsewhere(None)),
    ] {
        let pairing = Scripted::new(Saved::Approved(()), recorded);
        let code = Code::new(Ok(7));
        assert_eq!(enroll_once(&pairing, &code).await, Err(refused));
        assert!(pairing.done().is_empty());
        assert_eq!(code.reads(), 0);
    }
}

#[tokio::test]
async fn a_code_that_cannot_be_read_sends_and_records_nothing() {
    let pairing = Scripted::new(Saved::Nothing, None);
    let code = Code::new(Err("/run/secrets/code holds no pairing code".into()));
    assert_eq!(
        enroll_once(&pairing, &code).await,
        Err(NotEnrolled::Failed(
            "/run/secrets/code holds no pairing code".into()
        ))
    );
    assert!(pairing.done().is_empty());
}

#[tokio::test]
async fn an_address_that_cannot_be_recorded_sends_no_code() {
    let mut pairing = Scripted::new(Saved::Nothing, None);
    pairing.record = Err("paired-with could not be written: disk full".into());
    assert_eq!(
        enroll_once(&pairing, &Code::new(Ok(7))).await,
        Err(NotEnrolled::Failed(
            "paired-with could not be written: disk full".into()
        ))
    );
    assert_eq!(pairing.done(), ["record"]);
}

#[tokio::test]
async fn a_gateway_that_refuses_the_code_is_said() {
    let mut pairing = Scripted::new(Saved::Nothing, None);
    pairing.send = Err("the code is not the one the gateway gave".into());
    assert_eq!(
        enroll_once(&pairing, &Code::new(Ok(7))).await,
        Err(NotEnrolled::Failed(
            "the code is not the one the gateway gave".into()
        ))
    );
    assert_eq!(pairing.done(), ["record", "send 7"]);
}

#[tokio::test]
async fn a_pairing_that_cannot_be_read_enrolls_nothing() {
    let mut pairing = Scripted::new(Saved::Nothing, None);
    pairing.saved = Err("the pairing was not read".into());
    let code = Code::new(Ok(7));
    assert_eq!(
        enroll_once(&pairing, &code).await,
        Err(NotEnrolled::Failed("the pairing was not read".into()))
    );
    let mut pairing = Scripted::new(Saved::Pending, None);
    pairing.recorded = Err("paired-with cannot be read: denied".into());
    assert_eq!(
        enroll_once(&pairing, &code).await,
        Err(NotEnrolled::Failed(
            "paired-with cannot be read: denied".into()
        ))
    );
    assert!(pairing.done().is_empty());
    assert_eq!(code.reads(), 0);
}

#[tokio::test]
async fn enrolling_over_a_pairing_keeps_its_recorded_address() {
    // `env enroll` on a paired machine: the gateway client refuses it, and
    // the address the pairing recorded is not replaced first.
    let mut pairing = Scripted::new(Saved::Approved(()), Some("other.example:47650"));
    pairing.send = Err("this machine is already paired".into());
    assert_eq!(
        enroll(&pairing, 7).await,
        Err("this machine is already paired".into())
    );
    assert_eq!(pairing.done(), ["send 7"]);
}
