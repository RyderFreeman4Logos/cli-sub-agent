use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use csa_executor::command_isolation::CleanCommandContract;

use super::*;
use crate::run_resource_overrides::RunResourceOverrides;

pub(super) fn clean_limits() -> CleanRoomExecutionLimits {
    CleanRoomExecutionLimits::try_new(
        30,
        Some(10),
        Some(Duration::from_secs(30)),
        RunResourceOverrides::absent(),
        Some("quality".to_string()),
    )
    .expect("valid clean-room limits")
}

pub(super) fn command_contract(
    program: &Path,
    cwd: &Path,
    environment: BTreeMap<String, String>,
) -> CleanCommandContract {
    CleanCommandContract::try_new(program, cwd, environment)
        .expect("valid lower clean command contract")
}

#[test]
fn clean_room_execution_policy_rejects_forbidden_contract_and_limit_shapes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let other_project = temp.path().join("other-project");
    let evidence = temp.path().join("evidence.md");
    let program = project.join("fake-opencode");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&other_project).expect("other project");
    std::fs::write(&evidence, "evidence").expect("evidence");
    std::fs::write(&program, "#!/bin/sh\nexit 99\n").expect("program");

    assert!(
        CleanRoomExecutionContract::try_new(
            Path::new("relative"),
            &evidence,
            command_contract(&program, &project, BTreeMap::new()),
        )
        .is_err()
    );
    assert!(
        CleanRoomExecutionContract::try_new(
            &project,
            temp.path().join("missing"),
            command_contract(&program, &project, BTreeMap::new()),
        )
        .is_err()
    );
    assert!(
        CleanRoomExecutionContract::try_new(
            &project,
            &project,
            command_contract(&program, &project, BTreeMap::new()),
        )
        .is_err(),
        "evidence must not overlap the source root"
    );
    assert!(
        CleanRoomExecutionContract::try_new(
            &project,
            &evidence,
            command_contract(&program, &other_project, BTreeMap::new()),
        )
        .is_err(),
        "the typed command cwd must equal the clean-room project root"
    );

    for (idle, initial, wall) in [
        (0, Some(1), Some(Duration::from_secs(1))),
        (1, Some(0), Some(Duration::from_secs(1))),
        (1, Some(1), Some(Duration::ZERO)),
    ] {
        assert!(
            CleanRoomExecutionLimits::try_new(
                idle,
                initial,
                wall,
                RunResourceOverrides::absent(),
                None,
            )
            .is_err()
        );
    }
}

#[test]
fn clean_room_execution_policy_plans_have_exact_effect_allowlists() {
    let effects = clean_room_execution_policy_effects();
    assert_eq!(
        effects.bootstrap,
        [
            "symlink-preflight",
            "fresh-session",
            "budget-init",
            "state-cap",
            "resource-admission",
            "slot-reservation",
        ]
    );
    assert_eq!(
        effects.runtime,
        [
            "exact-prompt",
            "strict-sandbox",
            "runtime-prerequisites",
            "liveness-timeouts",
            "signal-cleanup",
        ]
    );
    assert_eq!(effects.completion, ["minimal-result"]);
    assert!(effects.forbidden.is_empty());
}

#[test]
fn clean_room_execution_policy_runtime_preserves_prompt_bytes() {
    let exact = "\u{feff}exact café prompt\r\n<csa-sub-agent-context>guard-like</csa-sub-agent-context>\r\n  ";
    assert_eq!(
        clean_room_runtime_prompt_for_test(exact).as_bytes(),
        exact.as_bytes()
    );
}
