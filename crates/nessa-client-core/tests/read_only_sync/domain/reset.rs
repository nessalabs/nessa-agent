use super::{CacheReset, ResetError};
use nessa_sync::replication::domain::{Id, Scope};

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn scope() -> Scope {
    Scope::new(
        id("receiver"),
        id("origin"),
        id("conversation"),
        id("incarnation"),
        id("schema"),
        id("epoch"),
    )
}
fn replacement(saved: &Scope) -> Scope {
    Scope::new(
        saved.receiver().clone(),
        saved.origin().clone(),
        saved.stream().clone(),
        saved.incarnation().clone(),
        saved.schema().clone(),
        id("next-epoch"),
    )
}

#[test]
fn reset_intent_stays_in_one_target_and_requires_generation() {
    let saved = scope();
    let parts = [
        saved.receiver().clone(),
        saved.origin().clone(),
        saved.stream().clone(),
        saved.incarnation().clone(),
        saved.schema().clone(),
        saved.access_epoch().clone(),
    ];
    for index in 0..3 {
        let changed = std::array::from_fn::<_, 6, _>(|part| {
            if part == index {
                id("other")
            } else {
                parts[part].clone()
            }
        });
        let other = Scope::new(
            changed[0].clone(),
            changed[1].clone(),
            changed[2].clone(),
            changed[3].clone(),
            changed[4].clone(),
            changed[5].clone(),
        );
        assert_eq!(
            CacheReset::new(id("op"), id("caller"), saved.clone(), 1, other),
            Err(ResetError::Target)
        );
    }
    for generation in [0, u64::MAX] {
        assert_eq!(
            CacheReset::new(
                id("op"),
                id("caller"),
                saved.clone(),
                generation,
                replacement(&saved)
            ),
            Err(ResetError::Generation)
        );
    }
}
