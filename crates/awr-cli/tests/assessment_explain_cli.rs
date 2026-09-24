//! DEC-021: optional CLI assessment explanations preserve legacy consumers.
use awr_core::Id;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

const WORK: &str = "work_items:\n- id: W\n  title: Prepare customer analysis\n  status: ready\n  next_action: Draft the analysis\n  depends_on: [D]\n  acceptance: [Deliver the reviewed analysis]\n- id: D\n  title: Required input\n  status: completed\n";
const RULE: &str = "# Authority {#authority severity=hard scope=project value=*}\n\nPreserve all hard source facts exactly.\n";
const SOURCES: &str = "[project]\nname='DEC021 fixture'\nexternal_key='dec021'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n[[sources]]\ndomain='goal'\nrole='primary'\npath='goal.md'\nadapter='markdown-heading-v1'\n[sources.options]\nstatus='active'\n[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-dec021-cli-{}", Id::new()));
        fs::create_dir_all(root.join("decisions")).unwrap();
        fs::write(root.join("work.yaml"), WORK).unwrap();
        fs::write(root.join("rules.md"), RULE).unwrap();
        fs::write(
            root.join("goal.md"),
            "# Deliver useful analysis\n\nPersist work and deliver reviewed customer results.\n",
        )
        .unwrap();
        fs::write(root.join("sources.toml"), SOURCES).unwrap();
        let f = Self(root);
        f.ok(&["init", "--manifest", "sources.toml", "--accept"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_awr"))
            .arg("--project")
            .arg(&self.0)
            .arg("--json")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let r = self.run(args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        serde_json::from_slice(&r.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn strip_explain(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove(awr_runtime::ASSESSMENT_EXPLAIN_FIELD);
    }
    value
}

#[test]
fn assessment_explain_capability_is_negotiable() {
    let out = Command::new(env!("CARGO_BIN_EXE_awr"))
        .env_clear()
        .args([
            "--json",
            "capabilities",
            "--require",
            "assessment.explain",
            "--require",
            "work.management",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let cap = value["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "assessment.explain")
        .unwrap();
    assert_eq!(cap["available"], true);
}

#[test]
fn prepare_explain_off_preserves_full_summary_action_and_on_attaches_same_chain() {
    let f = Fixture::new();
    let full = f.ok(&["work", "prepare", "W"]);
    assert!(full.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());
    let summary = f.ok(&["work", "prepare", "W", "--response-view", "summary"]);
    assert!(summary.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());
    assert_eq!(summary["response_view"]["view"], "summary");
    let action = f.ok(&["work", "prepare", "W", "--response-view", "action"]);
    assert!(action.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());
    assert_eq!(action["response_view"]["view"], "action");

    let explained = f.ok(&["work", "prepare", "W", "--explain"]);
    assert!(
        explained
            .get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD)
            .is_some()
    );
    let legacy = strip_explain(explained.clone());
    assert_eq!(legacy["ready"], full["ready"]);
    assert_eq!(
        legacy["management"]["decision"]["mode"],
        full["management"]["decision"]["mode"]
    );
    assert_eq!(
        legacy["management"]["decision"]["required_actions"],
        full["management"]["decision"]["required_actions"]
    );
    assert_eq!(
        legacy["context"]["work_context"]["context_hash"],
        full["context"]["work_context"]["context_hash"]
    );

    let explanation = &explained[awr_runtime::ASSESSMENT_EXPLAIN_FIELD];
    assert_eq!(explanation["capability"], "assessment.explain");
    assert_eq!(explanation["side_effects"]["claimed_work"], false);
    assert_eq!(explanation["side_effects"]["updated_completion"], false);
    assert_eq!(explanation["side_effects"]["auto_invoked_tools"], false);
    assert_eq!(explanation["side_effects"]["model_or_network"], false);
    assert_eq!(explanation["metrics"]["prepare_context_duplicated"], false);
    assert_eq!(
        explanation["metrics"]["explanation_embeds_rendered_context"],
        false
    );
    let rendered = full["context"]["work_context"]["rendered_context"]
        .as_str()
        .unwrap_or("");
    assert_eq!(
        explanation["metrics"]["rendered_context_bytes"],
        rendered.len()
    );
    let envelope_text = explanation["envelope"].to_string();
    assert!(!envelope_text.contains(rendered) || rendered.is_empty());

    let reattached = awr_runtime::attach_assessment_explanation(
        full.clone(),
        &awr_runtime::AttachExplanationOptions {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        explained[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"],
        reattached[awr_runtime::ASSESSMENT_EXPLAIN_FIELD]["envelope"]["assessment_hash"]
    );

    let wire = awr_runtime::wire_bytes(&explained).unwrap();
    let legacy_wire = awr_runtime::wire_bytes(&full).unwrap();
    assert!(wire > legacy_wire);
    assert!(explanation["metrics"]["envelope_bytes"].as_u64().unwrap() > 0);

    let action_explained = f.ok(&[
        "work",
        "prepare",
        "W",
        "--response-view",
        "action",
        "--explain",
    ]);
    assert_eq!(action_explained["response_view"]["view"], "action");
    assert!(action_explained.get("guidance").is_some());
    assert!(
        action_explained
            .get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD)
            .is_some()
    );

    let assess = f.ok(&["work", "assess", "W"]);
    assert!(assess.get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD).is_none());
    let assess_on = f.ok(&["work", "assess", "W", "--explain"]);
    assert!(
        assess_on
            .get(awr_runtime::ASSESSMENT_EXPLAIN_FIELD)
            .is_some()
    );
    assert_eq!(assess_on["decision"]["mode"], assess["decision"]["mode"]);
}

#[test]
fn explain_does_not_mutate_claim_or_completion_state() {
    let f = Fixture::new();
    let before = f.ok(&["work", "show", "W"]);
    let _ = f.ok(&["work", "prepare", "W", "--explain"]);
    let after = f.ok(&["work", "show", "W"]);
    assert_eq!(before["work"]["status"], after["work"]["status"]);
    assert_eq!(before["active_claims"], after["active_claims"]);
    assert_eq!(
        before["work"]["ordinary_completion"],
        after["work"]["ordinary_completion"]
    );
    assert!(
        after["active_claims"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
    );
}
