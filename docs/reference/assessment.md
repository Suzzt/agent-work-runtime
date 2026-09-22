# Engineering assessment contract (DEC-010)

This page freezes the first-batch **reuse mapping**, **compatibility surface**,
and **AssessmentEnvelope** contract for model-free engineering assessment.
It does not authorize a second work/session authority, a general scheduler, a
model client, or a new Decision object. Management intensity, claim admission,
source write, and completion remain the existing protocols.

Verified against tip `main` at execution:

| Field | Value |
| --- | --- |
| Verified main SHA | `8fa6679981c823842282a0a4b5b79b9c7165b2a4` |
| Branch tip when frozen | same SHA (worktree based on `main`) |
| Design snapshot (non-authoritative) | `e3bfac4ef0dd14b06f97eb4c972e90d62861a6d1` in the DEC pack |

The design pack's older SHA is inventory history only. Every reuse claim below
was re-checked against the verified main SHA above.

Machine-checkable companions live under
[`tests/fixtures/assessment/contracts/`](../../tests/fixtures/assessment/contracts/).

## 1. Tip inventory and deltas

### 1.1 Already on tip main (reuse; do not reimplement)

| Surface | Tip symbols / entry points | What it already decides |
| --- | --- | --- |
| Management intensity | `awr_core::{ManagementObservation,ManagementReason,ManagementMode,ManagementDecision,decide_management}`; `awr_runtime::{assess_management,manage_work}`; CLI `work assess` / `work manage`; MCP `awr_work_assess` / `awr_work_manage` | `undetermined` / `lightweight` / `continuous` from host assertions + runtime history; `unknown_observations`; `required_actions`; `reevaluation_signals`; never grants execution or changes completion policy |
| Prepare composition | `awr_runtime::prepare_work` embeds `management` beside readiness + context | Single snapshot with `ready`, `diagnostics`, `context`, `management`, continuity waits, and a stage `next_action` |
| Action rationale | `awr_core::ActionGuidance`; `awr_runtime::guide_prepared_work`; CLI/MCP `response_view=action` on prepare | One bounded `when` / `basis` / `next_action` / `recheck` card; omits only presentation duplicates; preserves incomplete diagnostics |
| Status queues | `awr_runtime::action_status(_page)` via `status --view action` | Four queues `current` / `ready` / `waiting` / `blocked` with recorded waits and non-terminal executions (not live process probes) |
| Context completeness | `awr_context::{ContextCompleteness,assess_completeness,hard_context,select_rules,budget_context}` | Required sources/rules/deps/acceptance; budget packing and omit reasons |
| Completion / evidence | `awr_core` completion validators; `prepare_completion` / `work complete` path | Source-bound evidence levels; unknown level is not a pass |
| Capability catalog | CLI `capabilities` id `work.management`; MCP tool list includes assess/manage/prepare | Advertise existing commands; build catalog is not project authorization |

### 1.2 Explicit deltas introduced by DEC-010

| Item | Tip today | DEC-010 freeze |
| --- | --- | --- |
| Shared AssessmentEnvelope | Separate JSON shapes per entry (`management` object, prepare envelope, action guidance, status queues) | One typed envelope schema for first-batch explain; maps existing returns without replacing them |
| Facts / observation / inference labels | Present as scattered strings (`observation_basis`, diagnostic codes, completeness issues) | Fixed classification vocabulary and field `state` / `basis` enums |
| Five semantic layers | Computed by different modules; not named as one layer set | Named layers with per-layer support status; unchecked layers are `not_evaluated`, never false |
| Explain delivery | Full assess JSON always; prepare embeds management; action view omits some management identity fields | Capability-negotiated explain field/view on **existing** prepare/assess paths; **no** new command name; **do not** occupy `decision show` |
| Unsupported / future fields | Absent keys | Explicit `unknown` (never zero-fill, never coerce to false/low-risk) |
| Scheduler / model client / Decision authority | Absent from assessment path | Remain out of scope for DEC-010 and the first batch |

Historical EVO-010 / EVO-012 / EVO-034 semantics that landed as the management,
prepare embedding, and status-action surfaces above are reused. DEC-010 does not
open a second rule engine for those conclusions.

## 2. First-batch scope

First batch (through the DEC-2 explain loop) **only**:

1. **Management assessment explanation** — typed wrap of the existing
   `ManagementDecision` / assess receipt.
2. **Action-rationale explanation** — typed wrap of existing `ActionGuidance`
   produced for prepare `response_view=action` (and the matching status action
   card shape).

Out of first batch for the DEC-010 envelope itself (later DEC cards; keep layers
as `not_evaluated` or field `state=unsupported` until implemented):

- Bounded fact-snapshot + source-quality labeling (DEC-011) — types and pure
  construction land in `awr_core::fact_snapshot` / `awr_runtime::fact_snapshot`;
  fixtures under `tests/fixtures/assessment/signals/`. WorkspaceFacts only via
  host supply or explicit collection; prepare does not run Git/AST/network.
- Typed AssessmentEnvelope compose + unknown/conflict semantics (DEC-012) —
  `awr_core::assessment_envelope` (`compose_assessment_envelope` /
  `evaluate_assessment`); fixtures under `tests/fixtures/assessment/envelope/`.
- Counterexample corpus (DEC-013)
- Composition pipeline (DEC-020) and CLI/MCP field injection (DEC-021)
- Context packing / retention explain (DEC-030+)
- AUTO / model routing (never core)

## 3. Five semantic layers

Layers are independent. A layer that was not run returns `not_evaluated`.
Missing evidence is `unknown` / `missing`, never `false`.

| Layer id | Tip authority today | First-batch envelope role |
| --- | --- | --- |
| `work_readiness` | `Store::work_readiness` diagnostics, active claims, dependency projection used by prepare/assess | Map readiness `ready` + diagnostic codes into assessments; do not invent readiness |
| `execution_admission` | Claim/session/permission paths; management sets `execution_admission: "not_granted_by_management_classification"`; team prepare reports `"not_evaluated"` | First batch records the tip constant / not_evaluated; never treat management continuous mode as admission |
| `context_completeness` | `ContextCompleteness` inside prepare `context` | Explain selected completeness flags and issues when present; otherwise `not_evaluated` |
| `delivery_observation` | Status waiting queue + non-terminal executions + MCP waits; host must query executors | Map recorded unknown outcomes / waits; never claim a live process was stopped |
| `completion_validity` | Completion/evidence validators and ordinary-confirmation policy | First batch only echoes that management does **not** change `completion_policy` (`unchanged_source_policy`); full validity checks stay on the completion path |

## 4. Facts, observations, and inference

Every signal in an envelope carries:

| Field | Allowed values / meaning |
| --- | --- |
| `state` | `known` | `missing` | `stale` | `conflicting` | `unsupported` | `unknown` |
| `basis` | `source_declared` | `runtime_recorded` | `host_asserted` | `locally_observed` | `rule_derived` |
| Classification | **fact** = source/runtime recorded value with identity; **observation** = host-asserted or locally observed, not independently verified; **inference** = rule_derived suggestion that must never be written as verified fact |

Tip mapping examples:

- `ManagementObservation.*` host fields → observation, `basis=host_asserted`, assess receipt `observation_basis=host_assertion_not_independently_verified`
- Runtime events (`mcp.wait_created`, `work.handoff`, `session.resumed`, non-terminal executions) → fact/observation with `basis=runtime_recorded` / reason basis `runtime_history` or `runtime_recorded_state`
- Source dependency / contract fingerprint mismatches → `basis=source_projection` / `source_declared` as appropriate
- `required_actions` / advisory suggestions → inference / advisory only; still require the next real admission check
- Omitted optional counters (`active_elapsed_ms`, `completed_rework_cycles`) → `missing` / listed in `unknown_observations`; never treated as zero risk

## 5. AssessmentEnvelope schema (frozen)

Schema id: `awr-assessment-envelope-v1`. Version: `1`. First-batch
`assessment_profile`: `management_and_action_rationale`.

```json
{
  "schema_id": "awr-assessment-envelope-v1",
  "schema_version": 1,
  "assessment_profile": "management_and_action_rationale",
  "identity": {
    "project_id": null,
    "work_key": null,
    "work_id": null,
    "branch_id": null,
    "contract_fingerprint": null,
    "policy_id": "tip_management_v1",
    "policy_version": 1,
    "input_summary": null,
    "as_of": null,
    "verified_main_sha": "8fa6679981c823842282a0a4b5b79b9c7165b2a4"
  },
  "layers": {
    "work_readiness": {"status": "not_evaluated", "summary": null, "basis_refs": []},
    "execution_admission": {"status": "not_evaluated", "summary": null, "basis_refs": []},
    "context_completeness": {"status": "not_evaluated", "summary": null, "basis_refs": []},
    "delivery_observation": {"status": "not_evaluated", "summary": null, "basis_refs": []},
    "completion_validity": {"status": "not_evaluated", "summary": null, "basis_refs": []}
  },
  "assessments": [],
  "evidence_quality": {
    "required_fields": [],
    "observed_fields": [],
    "missing": [],
    "stale": [],
    "conflicting": [],
    "host_asserted_only": [],
    "unsupported": [],
    "coverage_note": "coverage is a field count ratio, not probability"
  },
  "advisory_actions": [],
  "limits": {
    "candidates": null,
    "scan_ops": null,
    "return_bytes": null,
    "time_budget_ms": null,
    "read_scope": "assessment_read_only",
    "omitted_count": 0
  },
  "management": null,
  "action_rationale": null,
  "legacy": {
    "compatible": true,
    "source_views": [],
    "omitted_fields": []
  },
  "unsupported_fields": {}
}
```

Rules:

- `null` and `unknown` mean unset / not independently established. Do not
  substitute `0`, `false`, or empty-success.
- `assessments[].support` ∈ `supported` | `unknown` | `conflicting` | `unsupported`.
- `advisory_actions[].code` must come from the frozen advisory set below (machine
  enums, not free-form scripts).
- `unsupported_fields` maps dotted paths → `"unknown"` for any profile field the
  implementation does not yet populate.
- Evaluation is read-only: no model, network, thread farm, or claim/completion
  side effects from producing an envelope.

### 5.1 Management object (reuse mapping)

Maps 1:1 from tip `assess_management` / `ManagementDecision`:

| Envelope path | Tip source |
| --- | --- |
| `management.decision.version` | `ManagementDecision.version` (=1) |
| `management.decision.mode` | `undetermined` | `lightweight` | `continuous` |
| `management.decision.reasons[]` | `{code,basis,reference}` |
| `management.decision.unknown_observations[]` | unknown host fields |
| `management.decision.reevaluation_signals[]` | elapsed / rework signals |
| `management.decision.required_actions[]` | required maintenance strings |
| `management.decision.optional_maintenance[]` | optional strings (may be omitted in action view) |
| `management.decision.completion_policy` | always `unchanged_source_policy` |
| `management.decision.execution_admission` | always `not_granted_by_management_classification` |
| `management.contract_fingerprint` | assess receipt |
| `management.observation` / `observation_basis` / `admission_gaps` / `record_required` / `next_action` | assess receipt |

### 5.2 Action-rationale object

Maps from tip `ActionGuidance` / prepare action view / status action guidance:

| Envelope path | Tip source |
| --- | --- |
| `action_rationale.when` | `ActionGuidance.when` |
| `action_rationale.basis` | `ActionGuidance.basis` |
| `action_rationale.next_action` | `ActionGuidance.next_action` |
| `action_rationale.recheck` | `ActionGuidance.recheck` |
| `action_rationale.max_bytes` | `ACTION_GUIDANCE_MAX_BYTES` (1024) |
| `action_rationale.response_view` | `full` | `summary` | `action` when applicable |

## 6. Frozen reason codes and bases

### 6.1 Management reason `code` values (tip)

Host-assertion triggers from `decide_management`:

- `multiple_outcomes`
- `scope_requires_planning`
- `handoff_or_collaboration`
- `deferred_wait`
- `plan_invalidated`
- `query_outcome_before_retry`
- `independent_work_units`

Runtime / projection reasons from `assess_management`:

- `unresolved_dependencies` (`basis=source_projection`)
- `work_contract_changed` (`basis=source_projection`)
- `persistent_wait` (`basis=runtime_history`)
- `work_handoff` (`basis=runtime_history`)
- `cross_session_resume` (`basis=runtime_history`)
- `execution_result_requires_query` (`basis=runtime_recorded_state`)
- `continuous_management_retained` (`basis=runtime_history`)

Reason `basis` values observed on tip: `host_assertion`, `runtime_history`,
`source_projection`, `runtime_recorded_state`.

### 6.2 Unknown observation field names

When absent on the host observation: `single_outcome`, `bounded_scope`,
`single_executor`, `no_deferred_wait`, `plan_valid`, `outcome_known`,
`independently_schedulable_units`.

### 6.3 Reevaluation signals

- `active_elapsed_at_least_30_minutes` (when `active_elapsed_ms >= 30*60*1000`)
- `at_least_3_completed_rework_cycles` (when `completed_rework_cycles >= 3`)

These request reassessment only; they do **not** auto-promote lightweight work.

### 6.4 Required / optional action strings

Always required (all modes):

- `preserve_identity_intent_scope_and_current_state`
- `consume_required_context_and_hard_rules`
- `retain_completion_basis_and_actual_outcome`
- `check_source_versions_permissions_claims_and_request_identity`

Additional when `continuous`:

- `maintain_dependencies_and_ownership`
- `checkpoint_at_wait_handoff_interruption_and_phase_change`
- `maintain_next_action_wait_conditions_and_open_loops`
- `query_unknown_results_before_any_retry`

Conditional:

- `assess_unknown_management_facts` (undetermined + unknowns)
- `reevaluate_scope_plan_and_recovery_without_automatic_upgrade` (reevaluation signals)

### 6.5 First-batch advisory action codes

Closed set for envelope `advisory_actions` (aligns to tip required actions /
guidance; not executable scripts):

- `continue_prepare_current_work`
- `query_original_operation_result`
- `collect_user_reply`
- `refresh_sources`
- `repair_required_materials`
- `inspect_claim_conflict`
- `request_human_review`
- `record_management_observations`
- `no_advisory`

## 7. Read boundaries

Assessment reads may use:

- Already-authorized project store projections and event receipts
- The same read snapshot prepare/assess already opened
- Explicit caller-supplied host observations (labeled host-asserted)
- Fixture / frozen policy inputs for offline replay

Assessment reads must not:

- Open a model provider or any network scoring path
- Probe live OS processes to invent execution outcomes
- Run implicit `git` / AST / whole-repo scans beyond what prepare already did
- Mutate claims, sessions, sources, completion, or management records
  (recording observations remains the separate `work manage` mutation)
- Bypass source freshness / revision guards that the caller already faces
- Treat `decision show` content or markdown decision documents as this envelope

TOCTOU: an envelope is an explanation of a past snapshot. The next mutating
operation must re-check permissions, claims, and source versions.

## 8. Legacy output compatibility

| Existing consumer output | Compatibility strategy |
| --- | --- |
| `awr work assess` / `awr_work_assess` JSON | Remain authoritative for management. Envelope maps from it; do not rename tip fields. |
| `awr work prepare` / `awr_work_prepare` `management` object | Remain embedded. Explain adds an **optional** negotiated field/view; default `full` unchanged. |
| `response_view=summary` | Keep omitting only `context.work_context.selected_*` indexes as today. |
| `response_view=action` | Keep guidance card + existing omitted management identity/optional fields. Envelope action-rationale mirrors guidance without dropping required context. |
| `status --view action` queues | Unchanged; later layers may cite queue items as delivery_observation refs. |
| `decision show` / `object show` | **Not used** for assessment explain. Capability negotiation must not overload those commands. |
| MCP grouped tool contracts | New explain fields appear only when the client negotiates the capability / view; unnegotiated clients keep prior shapes. |

Prefer capability id `assessment.explain` (documented; wired in DEC-021) attached
to existing prepare/assess commands via an explain field or `response_view`
extension. Do **not** invent `work assess-explain` or similar for first batch.

## 9. Two read consumers

| Consumer | Tip entry | First-batch explain role |
| --- | --- | --- |
| CLI | `awr work assess`, `awr work prepare [--response-view …]`, `awr capabilities` | Same envelope bytes as MCP when inputs match; negotiate explain without new verbs |
| MCP | `awr_work_assess`, `awr_work_prepare` (`response_view` full/summary/action) | Same; reads persist nothing; SourceStale still requires explicit reindex |

Both consumers remain read-only for assessment. Closing MCP does not end sessions.
Unknown mutation outcomes still require querying the original request identity.

## 10. Unsupported fields → `unknown`

Any envelope field listed in the profile but not produced by the current
implementation is emitted under `unsupported_fields` with value `"unknown"`,
or as a layer/assessment with `status` / `support` = `not_evaluated` /
`unsupported`. Forbidden coercions:

- missing git diff ↛ `changed_lines=0`
- missing observation ↛ `false` risk absence
- unknown execution ↛ “safe to retry”
- rule score ↛ calibrated probability
- inference ↛ verified fact

## 11. Invariants

- `no_model_or_network_required`
- `source_first_single_work_authority`
- `reuse_existing_admission_and_completion`
- `unknown_is_not_false_or_zero`
- `evidence_coverage_is_not_probability`
- `no_second_decision_authority`
- `decision_show_not_occupied`

## 12. DEC-011 fact snapshot (bound inputs)

Schema id: `awr-fact-snapshot-v1`. Pure `build_fact_snapshot` accepts fixed
identity (project/work/branch, contract + source versions), `as_of`, scope,
truncation/limits, and labeled `FactSignal` values. Same-snapshot entity
version conflicts are rejected. Signal `state` /
`basis` / classification distinguish missing, stale, conflicting, unsupported,
host-asserted observations, and verified facts. Without a bound git diff,
`changed_lines` and low-risk labels stay unset (never `0` / `low`). Runtime
mapping reuses one prepared/assess read view (`scan_ops=1`) and does not
recurse the repository.

Machine fixtures: `tests/fixtures/assessment/signals/`.

## 13. DEC-012 typed envelope compose (unknown / conflict)

Schema id remains `awr-assessment-envelope-v1`. Pure
`compose_assessment_envelope` / `evaluate_assessment` accept fixed identity,
versioned `AssessmentPolicy`, `as_of`, optional `FactSnapshot`, optional
`ManagementDecision` / `ActionGuidance`, and labeled sub-assessments.

| Rule | Behavior |
| --- | --- |
| Support vocabulary | `supported` / `unknown` / `conflicting` / `unsupported` on each assessment; layers add `not_evaluated` |
| Preserve raw conclusions | Each sub-assessment keeps its original `conclusion`; compose does not rewrite unknown → false / low-risk |
| Conflict priority | Aggregate / layer merge uses Conflicting > Unknown > Unsupported > Supported > NotEvaluated |
| Reason order | Policy `reason_code_order` sorts reason codes and assessment surfacing deterministically (no expression DSL) |
| Hard gate (`hard_gate`) | `hard_reject` or hard reason codes → `Reject` / `Unknown`; heuristic scores never clear the gate |
| Evidence quality | `required` / `observed` / `missing` / `stale` / `conflicting` / `unsupported` + `applicability` separately; coverage note is a field-count ratio, not probability |
| Resource limits | Caps on assessments / advisory / candidates / return bytes; truncation sets `omitted_count` and must not claim Pass/full scan |
| Replay | Same input + policy + `as_of` → identical `assessment_hash` |
| Forbidden | Uncalibrated `confidence` / `probability` / `calibrated_probability` / `p_success` on conclusions |

Machine fixtures: `tests/fixtures/assessment/envelope/`.

## 14. Related pages



- [Management intensity](management.md) — classification rules hosts still follow
- [Workflow prepare](workflow.md) — prepare / completion preflight
- [Daily work](daily-work.md) — status action queues
- Fixtures: `tests/fixtures/assessment/contracts/`, `signals/`, `envelope/`
