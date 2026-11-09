//! Pure allocation history validation; serialization and files have separate tests.
use super::super::{AdmissionRefusal, AdmissionRejection, ObservedEnvironment, RefusalDisposition};
use super::{Assignment, EnvironmentIdentity};
fn admission() -> EnvironmentIdentity {
    EnvironmentIdentity::new(
        "11111111-1111-4111-8111-111111111111".into(),
        "nessa".into(),
        "22222222-2222-4222-8222-222222222222".into(),
        2,
    )
    .unwrap()
}
fn intent() -> Assignment {
    Assignment::Provisioning {
        thread: "thread".into(),
    }
}
fn assigned() -> Assignment {
    Assignment::Assigned {
        thread: "thread".into(),
        vm: "bx_fixture".into(),
        image: "image".into(),
        admission: admission(),
    }
}
fn refused(vm: &str) -> Assignment {
    refused_with(vm, "image", admission())
}
fn refused_with(vm: &str, image: &str, admission: EnvironmentIdentity) -> Assignment {
    Assignment::Refused {
        thread: "thread".into(),
        vm: vm.into(),
        image: image.into(),
        admission,
        disposition: RefusalDisposition::RollbackNewVm,
        rejection: AdmissionRejection::new(
            AdmissionRefusal::WrongVersion,
            Some(ObservedEnvironment {
                name: Some("nessa".into()),
                version: Some(3),
            }),
        ),
    }
}
#[test]
fn receipt_histories_preserve_targets_and_cannot_reactivate_terminal_refusals() {
    for history in [
        vec![intent()],
        vec![intent(), assigned()],
        vec![intent(), refused("bx_fixture")],
        vec![intent(), assigned(), refused("bx_fixture")],
    ] {
        Assignment::validate_history("thread", &history.iter().collect::<Vec<_>>()).unwrap();
    }
    for history in [
        vec![],
        vec![assigned()],
        vec![intent(), refused("bx_fixture"), assigned()],
        vec![intent(), assigned(), assigned()],
        vec![intent(), assigned(), refused("bx_other")],
        vec![
            intent(),
            assigned(),
            refused_with("bx_fixture", "other", admission()),
        ],
        vec![
            intent(),
            assigned(),
            refused_with(
                "bx_fixture",
                "image",
                EnvironmentIdentity::new(
                    "11111111-1111-4111-8111-111111111111".into(),
                    "nessa".into(),
                    "33333333-3333-4333-8333-333333333333".into(),
                    3,
                )
                .unwrap(),
            ),
        ],
        vec![
            intent(),
            assigned(),
            refused("bx_fixture"),
            refused("bx_fixture"),
        ],
    ] {
        assert!(
            Assignment::validate_history("thread", &history.iter().collect::<Vec<_>>()).is_err()
        );
    }
    assert!(Assignment::validate_history("different", &[&intent(), &assigned()]).is_err());
}
