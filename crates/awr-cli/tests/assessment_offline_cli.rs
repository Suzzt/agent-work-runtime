//! DEC-022: CLI developer entry points for offline replay / compare / kill-switch.
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, Output},
};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_awr"))
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn ok_json(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn assessment_capabilities_advertise_replay_shadow_and_advice_mode() {
    let out = bin()
        .env_clear()
        .args([
            "--json",
            "capabilities",
            "--require",
            "assessment.replay",
            "--require",
            "assessment.shadow_compare",
            "--require",
            "assessment.advice_mode",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = value["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["available"] == true)
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(ids.contains(&"assessment.replay"));
    assert!(ids.contains(&"assessment.shadow_compare"));
    assert!(ids.contains(&"assessment.advice_mode"));
}

#[test]
fn assessment_replay_compare_and_killswitch_cli_chain() {
    let fixture_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assessment/replay");
    let baseline = fixture_root.join("baseline-snapshot.json");
    let candidate = fixture_root.join("candidate-snapshot.json");
    assert!(
        baseline.is_file() && candidate.is_file(),
        "checked-in replay fixtures are required; tests must not rewrite them"
    );

    let missing = ok_json(&["--json", "assessment", "replay"]);
    assert_eq!(missing["report"]["status"], "not_replayable");
    assert!(missing["report"]["offline"].as_bool().unwrap());
    assert_eq!(missing["report"]["reread_production_state"], false);
    assert_eq!(missing["report"]["reran_tools"], false);
    assert_eq!(missing["report"]["model_or_network_requests"], false);

    let replayed = ok_json(
        [
            "--json",
            "assessment",
            "replay",
            "--snapshot",
            baseline.to_str().unwrap(),
        ]
        .as_slice(),
    );
    assert_eq!(replayed["report"]["status"], "replayed");
    assert!(replayed["report"]["assessment_hash"].as_str().is_some());

    let compared = ok_json(
        [
            "--json",
            "assessment",
            "compare",
            "--baseline",
            baseline.to_str().unwrap(),
            "--candidate",
            candidate.to_str().unwrap(),
        ]
        .as_slice(),
    );
    assert_eq!(compared["report"]["same_inputs"], true);
    assert_eq!(compared["report"]["execution_adoption"], false);
    assert_eq!(compared["report"]["context_adoption"], false);
    assert_eq!(compared["report"]["background_daemon"], false);

    let disabled = ok_json(&["--json", "assessment", "advice-mode", "--mode", "disabled"]);
    assert_eq!(disabled["mode"], "disabled");
    assert_eq!(disabled["effect"]["restores_prior_advice_behavior"], true);
    assert_eq!(disabled["effect"]["new_advice_attached"], false);
    assert_eq!(
        disabled["hard_protections_after_disable"]["claim_guards"],
        true
    );

    let shadow = ok_json(&["--json", "assessment", "advice-mode", "--mode", "shadow"]);
    assert_eq!(shadow["mode"], "shadow");
    assert_eq!(shadow["effect"]["background_daemon"], false);
    assert_eq!(shadow["effect"]["execution_adoption_changed"], false);
    assert_eq!(shadow["effect"]["context_adoption_changed"], false);
}
