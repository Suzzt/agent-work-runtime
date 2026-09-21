use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

fn matrix() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/reference/team-v1-evidence-matrix.json"
    ))
    .unwrap()
}

fn normalized_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    let value = value.as_str().ok_or_else(|| format!("{field} string"))?;
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(format!("{field} normalized non-empty string"));
    }
    Ok(value)
}

fn hex_string(value: &Value, length: usize, field: &str) -> Result<(), String> {
    let value = normalized_string(value, field)?;
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{field} hex length {length}"));
    }
    Ok(())
}

fn validate_live_agent_run(value: &Value) -> Result<(), String> {
    let require = |ok: bool, message: &str| if ok { Ok(()) } else { Err(message.to_string()) };
    let run = value.as_object().ok_or("live_agent_run object")?;
    require(
        run.get("schema_version") == Some(&json!(1)),
        "live schema version",
    )?;
    require(
        run.get("status") == Some(&json!("locally_verified")),
        "live verified status",
    )?;
    hex_string(
        run.get("git_head").ok_or("live git head missing")?,
        40,
        "live git head",
    )?;

    let clients = run
        .get("clients")
        .and_then(Value::as_array)
        .ok_or("live clients array")?;
    require(clients.len() == 2, "exactly two live clients")?;
    require(
        clients[0]["product"] == "Kimi Code CLI" && clients[1]["product"] == "ZCode CLI",
        "live client order",
    )?;
    normalized_string(&clients[0]["version"], "first client version")?;
    normalized_string(&clients[1]["version"], "second client version")?;
    let first_actor = normalized_string(&clients[0]["actor_id"], "first actor")?;
    let second_actor = normalized_string(&clients[1]["actor_id"], "second actor")?;
    require(first_actor != second_actor, "live actors must be distinct")?;

    normalized_string(
        run.get("project_key").ok_or("live project missing")?,
        "live project",
    )?;
    normalized_string(run.get("work_id").ok_or("live work missing")?, "live work")?;
    normalized_string(
        run.get("receipt_id").ok_or("live receipt missing")?,
        "live receipt",
    )?;
    normalized_string(
        run.get("execution_id").ok_or("live execution missing")?,
        "live execution",
    )?;
    normalized_string(run.get("flow").ok_or("live flow missing")?, "live flow")?;
    let evidence_path = normalized_string(
        run.get("evidence_path")
            .ok_or("live evidence locator missing")?,
        "live evidence locator",
    )?;
    require(
        Path::new(evidence_path)
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "unsafe live evidence locator",
    )?;

    let oracle = run
        .get("oracle")
        .and_then(Value::as_object)
        .ok_or("live oracle object")?;
    require(
        oracle.get("receipts").and_then(Value::as_u64) == Some(1),
        "live receipt count",
    )?;
    require(
        oracle.get("executions").and_then(Value::as_u64) == Some(1),
        "live execution count",
    )?;
    require(
        oracle.get("active_holder").and_then(Value::as_str) == Some(second_actor),
        "live successor holder",
    )?;
    require(
        oracle.get("pass").and_then(Value::as_bool) == Some(true),
        "live oracle pass",
    )?;
    require(
        run.get("not_a_release_tag").and_then(Value::as_bool) == Some(true),
        "live release limitation",
    )?;

    let review = run
        .get("historical_evidence_review")
        .and_then(Value::as_object)
        .ok_or("historical evidence review object")?;
    require(
        review.get("schema_version") == Some(&json!(1)),
        "historical evidence review schema version",
    )?;
    normalized_string(
        review
            .get("observed_at")
            .ok_or("historical evidence observation missing")?,
        "historical evidence observed_at",
    )?;
    hex_string(
        review
            .get("evidence_bundle_sha256")
            .ok_or("historical evidence bundle hash missing")?,
        64,
        "historical evidence bundle hash",
    )?;
    hex_string(
        review
            .get("retained_driver_sha256")
            .ok_or("retained driver hash missing")?,
        64,
        "retained driver hash",
    )?;
    require(
        review.get("retained_driver_binding") == Some(&json!("unbound")),
        "retained driver binding",
    )?;
    require(
        review.get("run_time") == Some(&Value::Null),
        "historical run time must remain unknown",
    )?;
    require(
        review.get("full_chain_reverified").and_then(Value::as_bool) == Some(false),
        "historical full chain was not reverified",
    )?;

    let current = run
        .get("current_verification")
        .and_then(Value::as_object)
        .ok_or("current verification object")?;
    require(
        current.get("live_rerun").and_then(Value::as_bool) == Some(false),
        "current live run was not rerun",
    )?;
    Ok(())
}

// This guard checks inventory, reference resolution and accounting. It never
// turns source inspection into an executed test or re-accepts historical runs.
fn validate(value: &Value) -> Result<(), String> {
    let require = |ok: bool, message: &str| if ok { Ok(()) } else { Err(message.to_string()) };
    require(value["schema_version"] == 2, "schema version")?;
    require(
        value["release_candidate"] == false && value["tag_pushed"] == false,
        "release flags",
    )?;
    let cases = value["cases"].as_array().ok_or("cases array")?;
    let expected: BTreeSet<String> = (1..=69).map(|n| format!("TC-{n:03}")).collect();
    let ids: BTreeSet<String> = cases
        .iter()
        .map(|c| c["id"].as_str().unwrap_or("").to_owned())
        .collect();
    require(
        cases.len() == ids.len() && ids == expected,
        "exact unique TC-001..TC-069 inventory",
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut required = 0;
    let mut implemented = 0;
    let mut accepted = 0;
    let mut partial = 0;
    let mut pending = 0;
    for case in cases {
        required += usize::from(case["required"].as_bool().ok_or("required boolean")?);
        implemented += usize::from(
            case["protocol_implemented"]
                .as_bool()
                .ok_or("implemented boolean")?,
        );
        let replayed = case["status"] == "real_agent_accepted";
        require(
            case["real_agent_clients"] == replayed,
            "agent status mismatch",
        )?;
        if replayed {
            accepted += 1;
            require(
                case["agent_evidence"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty()),
                "agent evidence missing",
            )?;
        } else {
            require(
                case["status"] == "automated_evidence_pending",
                "unsupported acceptance status",
            )?;
            require(
                case["blocker"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty()),
                "pending blocker missing",
            )?;
            pending += 1;
        }
        require(
            case["automated_coverage"]["status"] == "partial",
            "unsupported coverage claim",
        )?;
        require(
            case["automated_coverage"]["gap"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()),
            "coverage gap missing",
        )?;
        partial += 1;
        let tests = case["automated_tests"]
            .as_array()
            .ok_or("test references array")?;
        require(!tests.is_empty(), "missing partial test reference")?;
        let mut has_pg = false;
        for test in tests {
            let path = test["path"].as_str().ok_or("test path")?;
            require(
                Path::new(path)
                    .components()
                    .all(|p| matches!(p, Component::Normal(_))),
                "unsafe reference path",
            )?;
            let package = test["package"].as_str().ok_or("package")?;
            let target = test["target"].as_str().ok_or("target")?;
            require(
                path == format!("crates/{package}/tests/{target}.rs"),
                "target/path mismatch",
            )?;
            let source = std::fs::read_to_string(root.join(path))
                .map_err(|_| format!("missing test {path}"))?;
            require(
                test["test_file_sha256"]
                    == format!(
                        "{:x}",
                        Sha256::digest(source.replace("\r\n", "\n").as_bytes())
                    ),
                "stale test source fingerprint",
            )?;
            let file = syn::parse_file(&source).map_err(|_| format!("invalid Rust {path}"))?;
            let function = test["function"].as_str().ok_or("function")?;
            let found = file.items.iter().any(|item| match item {
                syn::Item::Fn(f) => {
                    f.sig.ident == function
                        && f.attrs.iter().any(|a| {
                            let parts: Vec<_> = a
                                .path()
                                .segments
                                .iter()
                                .map(|s| s.ident.to_string())
                                .collect();
                            parts == ["test"] || parts == ["tokio", "test"]
                        })
                }
                _ => false,
            });
            require(found, &format!("not a test: {path}::{function}"))?;
            let level = test["level"].as_str().ok_or("level")?;
            require(
                matches!(
                    level,
                    "postgres_integration" | "pure_function" | "cli_process"
                ),
                "unknown level",
            )?;
            has_pg |= level == "postgres_integration";
            require(
                test["features"]
                    == if package == "awr-team-pg" {
                        json!(["pg-tests"])
                    } else {
                        json!([])
                    },
                "test features",
            )?;
            require(
                test["key_assertion"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty()),
                "key assertion missing",
            )?;
            require(
                test["reviewed_source_sha"]
                    .as_str()
                    .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())),
                "review source SHA",
            )?;
            // This version intentionally contains reference mappings only.
            // Executed coverage needs a separate verifiable receipt contract.
            require(
                test.get("last_run") == Some(&Value::Null),
                "unbound execution claim",
            )?;
        }
        require(
            case["real_postgresql"] == has_pg,
            "PostgreSQL level mismatch",
        )?;
    }
    require(required == 69, "all contract cases are required")?;
    require(
        value["counts"]
            == json!({
                "required": required, "protocol_implemented": implemented,
                "real_agent_accepted": accepted, "automated_partial": partial,
                "automated_evidence_pending": pending
            }),
        "derived counts mismatch",
    )?;
    validate_live_agent_run(&value["live_agent_run"])?;
    Ok(())
}

#[test]
fn evidence_matrix_has_resolvable_references_and_derived_counts() {
    validate(&matrix()).unwrap();
}

#[test]
fn mutations_cannot_hide_missing_cases_or_fabricate_coverage() {
    let original = matrix();
    for (name, change) in [
        ("duplicate", 0),
        ("outside inventory", 1),
        ("missing file", 2),
        ("implementation count", 3),
        ("required count", 4),
        ("missing case", 5),
        ("release flag", 6),
        ("missing function", 7),
        ("helper is not test", 8),
        ("level mismatch", 9),
        ("fabricated run", 10),
        ("stale fingerprint", 11),
    ] {
        let mut bad = original.clone();
        match change {
            0 => bad["cases"][68]["id"] = json!("TC-001"),
            1 => bad["cases"][68]["id"] = json!("TC-999"),
            2 => {
                bad["cases"][0]["automated_tests"][0]["path"] =
                    json!("crates/awr-team-pg/tests/missing.rs");
                bad["cases"][0]["automated_tests"][0]["target"] = json!("missing")
            }
            3 => {
                for c in bad["cases"].as_array_mut().unwrap() {
                    c["protocol_implemented"] = json!(false);
                }
            }
            4 => {
                for c in bad["cases"].as_array_mut().unwrap() {
                    c["required"] = json!(false);
                }
            }
            5 => {
                bad["cases"].as_array_mut().unwrap().pop();
            }
            6 => bad["release_candidate"] = json!(true),
            7 => bad["cases"][0]["automated_tests"][0]["function"] = json!("imaginary_test"),
            8 => bad["cases"][0]["automated_tests"][0]["function"] = json!("setup"),
            9 => bad["cases"][0]["real_postgresql"] = json!(false),
            10 => bad["cases"][0]["automated_tests"][0]["last_run"] = json!({"passed": true}),
            11 => {
                bad["cases"][0]["automated_tests"][0]["test_file_sha256"] =
                    json!("0000000000000000000000000000000000000000000000000000000000000000")
            }
            _ => unreachable!(),
        }
        assert!(validate(&bad).is_err(), "mutation passed: {name}");
    }
}

#[test]
fn legitimate_acceptance_and_implementation_changes_use_derived_counts() {
    let mut value = matrix();
    value["cases"][0]["status"] = json!("automated_evidence_pending");
    value["cases"][0]["real_agent_clients"] = json!(false);
    value["cases"][0]["blocker"] = json!("Historical acceptance under re-review");
    value["cases"][0]["protocol_implemented"] = json!(false);
    value["counts"]["real_agent_accepted"] = json!(41);
    value["counts"]["automated_evidence_pending"] = json!(28);
    value["counts"]["protocol_implemented"] = json!(66);
    validate(&value).unwrap();
}

#[test]
fn live_agent_run_accepts_new_well_formed_observation_identifiers() {
    let mut value = matrix();
    let run = &mut value["live_agent_run"];
    run["git_head"] = json!("0123456789abcdef0123456789abcdef01234567");
    run["clients"][0]["version"] = json!("2.1.0");
    run["clients"][0]["actor_id"] = json!("kimi-successor-flow-a");
    run["clients"][1]["version"] = json!("0.17.0");
    run["clients"][1]["actor_id"] = json!("zcode-successor-flow-b");
    run["project_key"] = json!("p11-live-next");
    run["work_id"] = json!("work-p11-next");
    run["receipt_id"] = json!("receipt-next");
    run["execution_id"] = json!("execution-next");
    run["evidence_path"] = json!("ledger/evidence/TEAM-P11/live-dual-cli-next.json");
    run["oracle"]["active_holder"] = json!("zcode-successor-flow-b");
    run["historical_evidence_review"]["evidence_bundle_sha256"] = json!("a".repeat(64));
    run["historical_evidence_review"]["retained_driver_sha256"] = json!("b".repeat(64));
    validate(&value).unwrap();
}

#[test]
fn live_agent_run_mutations_cannot_fabricate_a_successful_handoff() {
    let original = matrix();
    for name in [
        "receipts zero",
        "receipts two",
        "executions zero",
        "executions two",
        "holder reverted to first actor",
        "actors equal",
        "actor whitespace pseudo difference",
        "actor control character",
        "receipt empty",
        "execution empty",
        "evidence empty",
        "evidence traversal",
        "oracle failed",
        "wrong product",
        "release limitation missing",
        "run structure type",
        "clients structure type",
        "oracle structure type",
        "required field missing",
        "receipt count structure type",
        "git head malformed",
        "provenance missing",
        "provenance hash malformed",
        "retained driver falsely bound",
        "historical run time fabricated",
        "historical chain falsely reverified",
        "current run falsely claimed",
    ] {
        let mut bad = original.clone();
        match name {
            "receipts zero" => bad["live_agent_run"]["oracle"]["receipts"] = json!(0),
            "receipts two" => bad["live_agent_run"]["oracle"]["receipts"] = json!(2),
            "executions zero" => bad["live_agent_run"]["oracle"]["executions"] = json!(0),
            "executions two" => bad["live_agent_run"]["oracle"]["executions"] = json!(2),
            "holder reverted to first actor" => {
                bad["live_agent_run"]["oracle"]["active_holder"] = json!("kimi-cli")
            }
            "actors equal" => bad["live_agent_run"]["clients"][1]["actor_id"] = json!("kimi-cli"),
            "actor whitespace pseudo difference" => {
                bad["live_agent_run"]["clients"][1]["actor_id"] = json!("kimi-cli ")
            }
            "actor control character" => {
                bad["live_agent_run"]["clients"][1]["actor_id"] = json!("zcode\ncli")
            }
            "receipt empty" => bad["live_agent_run"]["receipt_id"] = json!(""),
            "execution empty" => bad["live_agent_run"]["execution_id"] = json!(""),
            "evidence empty" => bad["live_agent_run"]["evidence_path"] = json!(""),
            "evidence traversal" => {
                bad["live_agent_run"]["evidence_path"] = json!("../private/live.json")
            }
            "oracle failed" => bad["live_agent_run"]["oracle"]["pass"] = json!(false),
            "wrong product" => bad["live_agent_run"]["clients"][0]["product"] = json!("Other CLI"),
            "release limitation missing" => {
                bad["live_agent_run"]["not_a_release_tag"] = json!(false)
            }
            "run structure type" => bad["live_agent_run"] = json!([]),
            "clients structure type" => bad["live_agent_run"]["clients"] = json!({}),
            "oracle structure type" => bad["live_agent_run"]["oracle"] = json!([]),
            "required field missing" => {
                bad["live_agent_run"]
                    .as_object_mut()
                    .unwrap()
                    .remove("work_id");
            }
            "receipt count structure type" => {
                bad["live_agent_run"]["oracle"]["receipts"] = json!("1")
            }
            "git head malformed" => bad["live_agent_run"]["git_head"] = json!("main"),
            "provenance missing" => {
                bad["live_agent_run"]
                    .as_object_mut()
                    .unwrap()
                    .remove("historical_evidence_review");
            }
            "provenance hash malformed" => {
                bad["live_agent_run"]["historical_evidence_review"]["evidence_bundle_sha256"] =
                    json!("unknown")
            }
            "retained driver falsely bound" => {
                bad["live_agent_run"]["historical_evidence_review"]["retained_driver_binding"] =
                    json!("bound")
            }
            "historical run time fabricated" => {
                bad["live_agent_run"]["historical_evidence_review"]["run_time"] =
                    json!("2026-09-17T00:00:00Z")
            }
            "historical chain falsely reverified" => {
                bad["live_agent_run"]["historical_evidence_review"]["full_chain_reverified"] =
                    json!(true)
            }
            "current run falsely claimed" => {
                bad["live_agent_run"]["current_verification"]["live_rerun"] = json!(true)
            }
            _ => unreachable!(),
        }
        assert!(validate(&bad).is_err(), "mutation passed: {name}");
    }
}
