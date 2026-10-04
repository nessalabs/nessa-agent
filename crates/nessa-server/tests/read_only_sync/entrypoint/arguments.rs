//! Parsed online requests carry no invented source identity.
use super::*;
use uuid::Uuid;
#[test]
fn explicit_online_counts_and_arity_are_checked_before_composition() {
    let target = Uuid::new_v4().to_string();
    for pages in ["0", "1", "123"] {
        let args = ["sync-records", "cache", "profile", &target, pages].map(str::to_owned);
        assert!(matches!(parse(&args), Ok(Command::Records { .. })));
    }
    for pages in ["", "-1", "+1", " 1", "1.0", "184467440737095516160"] {
        let args = ["sync-records", "cache", "profile", &target, pages].map(str::to_owned);
        assert!(matches!(parse(&args), Err(CommandError::Arguments)));
    }
    let args = ["check-records", "cache", "profile", &target].map(str::to_owned);
    assert!(matches!(
        parse(&args),
        Ok(Command::Records { pages: 0, .. })
    ));
    let extra = ["check-records", "cache", "profile", &target, "1"].map(str::to_owned);
    assert!(matches!(parse(&extra), Err(CommandError::Arguments)));
    let args = ["sync-catalogue", "cache", "profile", "0"].map(str::to_owned);
    assert!(matches!(
        parse(&args),
        Ok(Command::Catalogue { pages: 0, .. })
    ));
}

#[test]
fn device_commands_take_only_a_profile() {
    let pair = ["pair", "profile"].map(str::to_owned);
    assert!(matches!(parse(&pair), Ok(Command::Pair { .. })));
    let status = ["status", "cache", "profile"].map(str::to_owned);
    assert!(matches!(parse(&status), Ok(Command::Status { .. })));
    let uncached = ["status", "profile"].map(str::to_owned);
    assert!(matches!(parse(&uncached), Err(CommandError::Arguments)));
    // The code is never an argument.
    let code = ["pair", "profile", "ABCD-EFGH"].map(str::to_owned);
    assert!(matches!(parse(&code), Err(CommandError::Arguments)));
    let bare = ["status"].map(str::to_owned);
    assert!(matches!(parse(&bare), Err(CommandError::Arguments)));
}
