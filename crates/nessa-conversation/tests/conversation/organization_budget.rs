use super::*;

#[test]
fn a_budget_refuses_past_its_bound_and_gives_a_place_back_when_it_is_let_go() {
    let budgets = OrganizationBudgets::new(Some(2), Some(1));
    let first = budgets.lease().expect("room for one");
    let second = budgets.lease().expect("room for two");
    assert!(budgets.lease().is_none(), "a third is past the bound");
    let command = budgets.command().expect("commands are counted apart");
    assert!(budgets.command().is_none());
    assert_eq!(budgets.running(), (2, 1));
    drop(first);
    assert_eq!(budgets.running(), (1, 1));
    let third = budgets
        .lease()
        .expect("the place given back is taken again");
    drop((second, third, command));
    assert_eq!(budgets.running(), (0, 0));
}

#[test]
fn no_bound_counts_without_refusing() {
    let budgets = OrganizationBudgets::default();
    let places: Vec<_> = (0..1_000).filter_map(|_| budgets.lease()).collect();
    assert_eq!(places.len(), 1_000);
    assert_eq!(budgets.running(), (1_000, 0));
    let unbounded = OrganizationBudgets::new(None, None);
    assert!(unbounded.command().is_some());
}

#[test]
fn clones_share_one_budget() {
    let budgets = OrganizationBudgets::new(Some(1), None);
    let held = budgets.clone().lease().expect("room");
    assert!(budgets.lease().is_none(), "the clone's place counts here");
    drop(held);
    assert!(budgets.lease().is_some());
}
