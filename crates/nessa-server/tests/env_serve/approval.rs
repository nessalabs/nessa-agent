use super::{approval, Answer, EnvironmentPairing, NotApproved, Saved};
use std::{sync::Mutex, time::Duration};
use tokio::sync::watch;

const POLL: Duration = Duration::from_secs(5);

/// A pairing that answers from a script: each read takes the next saved
/// state (the last one repeats), each ask the next answer, which may ask
/// stop as it is given.
struct Scripted {
    saved: Mutex<Vec<Result<Saved<u32>, String>>>,
    answers: Mutex<Vec<Answer>>,
    asked: Mutex<u32>,
    stop_when_asked: Option<watch::Sender<bool>>,
}

impl Scripted {
    fn new(saved: Vec<Result<Saved<u32>, String>>, answers: Vec<Answer>) -> Self {
        let (mut saved, mut answers) = (saved, answers);
        saved.reverse();
        answers.reverse();
        Self {
            saved: Mutex::new(saved),
            answers: Mutex::new(answers),
            asked: Mutex::new(0),
            stop_when_asked: None,
        }
    }

    fn asked(&self) -> u32 {
        *self.asked.lock().unwrap()
    }
}

impl EnvironmentPairing for Scripted {
    type Approval = u32;

    async fn saved(&self) -> Result<Saved<u32>, String> {
        let mut saved = self.saved.lock().unwrap();
        if saved.len() > 1 {
            saved.pop().unwrap()
        } else {
            match saved.last().unwrap() {
                Ok(Saved::Approved(approval)) => Ok(Saved::Approved(*approval)),
                Ok(Saved::Pending) => Ok(Saved::Pending),
                Ok(Saved::Nothing) => Ok(Saved::Nothing),
                Err(reason) => Err(reason.clone()),
            }
        }
    }

    async fn ask(&self) -> Answer {
        *self.asked.lock().unwrap() += 1;
        if let Some(stop) = &self.stop_when_asked {
            stop.send(true).unwrap();
        }
        self.answers
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(Answer::Unanswered("script ran out".into()))
    }
}

fn running() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

#[tokio::test(start_paused = true)]
async fn an_approved_pairing_is_used_without_asking() {
    let pairing = Scripted::new(vec![Ok(Saved::Approved(7))], Vec::new());
    let (_stop, stopping) = running();
    assert_eq!(approval(&pairing, POLL, stopping).await, Ok(Some(7)));
    assert_eq!(pairing.asked(), 0);
}

#[tokio::test(start_paused = true)]
async fn nothing_paired_stops_without_asking_the_gateway() {
    let pairing = Scripted::new(vec![Ok(Saved::Nothing)], Vec::new());
    let (_stop, stopping) = running();
    assert_eq!(
        approval(&pairing, POLL, stopping).await,
        Err(NotApproved::NotPaired)
    );
    assert_eq!(pairing.asked(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_pairing_that_cannot_be_read_stops_with_its_reason() {
    let pairing = Scripted::new(vec![Err("corrupt".into())], Vec::new());
    let (_stop, stopping) = running();
    assert_eq!(
        approval(&pairing, POLL, stopping).await,
        Err(NotApproved::Unusable("corrupt".into()))
    );
}

/// No answer, a failed save among them, and waiting are asked again every
/// poll; approval is then read from what the pairing saved.
#[tokio::test(start_paused = true)]
async fn waiting_and_no_answer_are_asked_again_until_approved() {
    let pairing = Scripted::new(
        vec![
            Ok(Saved::Pending),
            Ok(Saved::Pending),
            Ok(Saved::Pending),
            Ok(Saved::Pending),
            Ok(Saved::Approved(3)),
        ],
        vec![
            Answer::Unanswered("unreachable".into()),
            Answer::Waiting,
            Answer::Unanswered("credential not saved".into()),
            Answer::Approved,
        ],
    );
    let (_stop, stopping) = running();
    let started = tokio::time::Instant::now();
    assert_eq!(approval(&pairing, POLL, stopping).await, Ok(Some(3)));
    assert_eq!(pairing.asked(), 4);
    // Three waits, none after the approval.
    assert_eq!(started.elapsed(), POLL * 3);
}

#[tokio::test(start_paused = true)]
async fn ended_not_paired_and_unusable_answers_stop_waiting() {
    for (answer, stopped) in [
        (Answer::Ended, NotApproved::Ended),
        (Answer::NotPaired, NotApproved::NotPaired),
        (
            Answer::Unusable("phase".into()),
            NotApproved::Unusable("phase".into()),
        ),
    ] {
        let pairing = Scripted::new(vec![Ok(Saved::Pending)], vec![answer]);
        let (_stop, stopping) = running();
        assert_eq!(approval(&pairing, POLL, stopping).await, Err(stopped));
    }
}

/// Codex review on #756: stop asked while the gateway approves is seen
/// first, so nothing is composed after a stop.
#[tokio::test(start_paused = true)]
async fn stop_asked_as_approval_arrives_wins() {
    let (stop, stopping) = running();
    let mut pairing = Scripted::new(
        vec![Ok(Saved::Pending), Ok(Saved::Approved(1))],
        vec![Answer::Approved],
    );
    pairing.stop_when_asked = Some(stop);
    assert_eq!(approval(&pairing, POLL, stopping).await, Ok(None));
    assert_eq!(pairing.asked(), 1);
}

#[tokio::test(start_paused = true)]
async fn stop_asked_while_waiting_ends_the_wait() {
    let pairing = Scripted::new(vec![Ok(Saved::Pending)], vec![Answer::Waiting]);
    let (stop, stopping) = running();
    let waiting = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        stop.send(true).unwrap();
    };
    let started = tokio::time::Instant::now();
    let (approved, ()) = tokio::join!(approval(&pairing, POLL, stopping), waiting);
    assert_eq!(approved, Ok(None));
    assert_eq!(started.elapsed(), Duration::from_secs(1));
}
