//! The table is the contract the client also compiles in, so what it says has
//! to be what composition actually injects.
use super::{kill_timeout, shutdown_grace, startup_timeout, BUDGETS_JSON};
use std::time::Duration;

#[test]
fn the_injected_budgets_are_the_ones_the_table_states() {
    let table: serde_json::Value =
        serde_json::from_str(BUDGETS_JSON).expect("bundled budgets table must parse");
    let agent = &table["agent"];
    assert_eq!(
        startup_timeout(),
        Duration::from_millis(agent["startupMs"].as_u64().expect("startupMs"))
    );
    assert_eq!(
        shutdown_grace(),
        Duration::from_millis(agent["shutdownGraceMs"].as_u64().expect("shutdownGraceMs"))
    );
    assert_eq!(
        kill_timeout(),
        Duration::from_millis(agent["killTimeoutMs"].as_u64().expect("killTimeoutMs"))
    );
}

/// The client's deadline is derived from these three plus a margin, so a table
/// whose values do not add up to a waitable interval breaks the other side too.
#[test]
fn every_budget_is_a_positive_interval() {
    for budget in [startup_timeout(), shutdown_grace(), kill_timeout()] {
        assert!(
            !budget.is_zero(),
            "a zero budget expires before it is waited on"
        );
    }
}
