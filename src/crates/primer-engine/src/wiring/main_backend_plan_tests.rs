use super::{MainBackendPlan, plan_main_backend};

#[test]
fn primary_ok_no_fallback_is_primary_alone() {
    assert_eq!(
        plan_main_backend(true, false, false),
        MainBackendPlan::PrimaryAlone
    );
    assert_eq!(
        plan_main_backend(true, false, true),
        MainBackendPlan::PrimaryAlone
    );
}

#[test]
fn primary_ok_fallback_built_is_wrapped() {
    assert_eq!(
        plan_main_backend(true, true, true),
        MainBackendPlan::Wrapped
    );
}

#[test]
fn primary_ok_fallback_failed_is_primary_alone() {
    assert_eq!(
        plan_main_backend(true, true, false),
        MainBackendPlan::PrimaryAlone
    );
}

#[test]
fn primary_failed_secondary_built_is_secondary_alone() {
    assert_eq!(
        plan_main_backend(false, true, true),
        MainBackendPlan::SecondaryAlone
    );
}

#[test]
fn primary_failed_secondary_failed_is_fail() {
    assert_eq!(plan_main_backend(false, true, false), MainBackendPlan::Fail);
}

#[test]
fn primary_failed_no_fallback_is_fail() {
    assert_eq!(
        plan_main_backend(false, false, false),
        MainBackendPlan::Fail
    );
    assert_eq!(plan_main_backend(false, false, true), MainBackendPlan::Fail);
}
