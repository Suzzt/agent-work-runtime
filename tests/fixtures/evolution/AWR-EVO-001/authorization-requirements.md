# AWR-EVO-001 authorization requirements

> Frozen at `2026-09-23T06:56:54+08:00` (Asia/Taipei). Plan hash: `sha256:9c2565ea067d332b549420b75d36fc22b7567d28a0f70e9b75673386f4e37958`.
> This item only formulates the experiment. It does **not** call paid models or start native agents.

## 1. Evidence layers (non-substituting)

| Layer | May run in EVO-001 without extra auth? | Proves | Does not prove |
| --- | --- | --- | --- |
| Simulated | Yes | Fixture/plan structure | Native host consumption, paid savings |
| Protocol | Yes | Fixed program behavior / counterexamples | Named host handoff, real invoice savings |
| Native | **No** — needs `native_verification` block | Named host/version consume or handoff | Full Studio integration without product |
| Paid | **No** — needs `paid_verification` block | Authorized model trials with real fees | Unlimited budget or cherry-picked best |

Layers cannot substitute for each other. Config files, schemas, model self-reports, and CLI exit codes alone do not prove business completion.

## 2. Independent authorization fields

### 2.1 `native_verification` (gates AWR-EVO-054 and RET-006 R2 native)

All fields required before any native agent/host verification starts:

| Field | Meaning |
| --- | --- |
| `host_a_id_and_version` | Named host A and exact version |
| `host_b_id_and_version` | Named host B and exact version (if dual-host) |
| `machine_id` | Machine allowed for the run |
| `account_id` | Account identity (not merely “a key exists”) |
| `session_policy` | `new` / `switch` / explicit window ops allowed |
| `controlled_stop_allowed` | Whether stopping controlled executions is permitted |
| `project_and_data_scope` | Exact project/data boundaries |
| `fee_ceiling_total` | Total fee ceiling (empty ≠ unlimited) |
| `fee_ceiling_per_trial` | Per-trial fee ceiling |
| `reviewer_id` | Human reviewer responsible |
| `evidence_publication_scope` | What may be published vs kept private |
| `authorized_by` | Explicit authorizer identity |
| `authorized_at` | Timestamp of authorization |
| `authorization_id` | Unique authorization record id |

**Explicit non-approvals:** API key availability, existing windows, and past authorizations do **not** satisfy this block.

### 2.2 `paid_verification` (gates AWR-EVO-060 and RET-006 R2 paid)

All fields required before any paid model trial starts:

| Field | Meaning |
| --- | --- |
| `final_case_list_approved` | Approved final case IDs (subset of frozen VC-* / RET slots) |
| `trial_count_approved` | Approved trial count ≤ registered denominator |
| `budget_total` | Total budget; **empty is not unlimited** |
| `budget_per_trial` | Per-trial budget |
| `provider_and_model_ids` | Exact provider/model identities |
| `account_id` | Billing account |
| `client_or_host_id` | Client/host binding the spend |
| `window_ops_allowed` | Allowed session/window operations |
| `reviewer_id` | Human reviewer |
| `evidence_publication_scope` | Publication boundary |
| `authorized_by` | Explicit authorizer identity |
| `authorized_at` | Timestamp of authorization |
| `authorization_id` | Unique authorization record id |

**Over-budget rule:** Stop starting new trials; keep unrun slots in the denominator. Do not run until satisfied and pick the best results.

## 3. Current authorization state (EVO-001 freeze)

```json
{
  "native_verification": {
    "authorized": false,
    "fields_present": {},
    "starts_allowed": false,
    "reason": "No explicit native authorization fields provided at EVO-001 freeze."
  },
  "paid_verification": {
    "authorized": false,
    "fields_present": {},
    "starts_allowed": false,
    "reason": "No explicit paid authorization fields provided at EVO-001 freeze."
  },
  "key_availability_is_approval": false,
  "real_materials_authorized": false
}
```

## 4. Evaluation axes (cannot substitute)

1. **Implementation correctness** (`AX-CORRECTNESS`)
2. **Performance gain** (`AX-PERF`) — separately measure background compute, response bytes, stable prefix, actual cached usage, and full bill
3. **Full-workflow incremental value** (`AX-VALUE`)

A pass on one axis never marks another axis met.

## 5. RET-AMD-001 / RET-006

Independent R0/R1/R2 matrix is frozen in `evaluation-plan.json` → `ret_amd_001`.
The old 36-trial EVO-060 suggestion/budget does **not** auto-expand into RET-006.

## 6. Minimal counterexample checks (pre-registered)

- Tokenizer change
- Billing unknown
- Referee material isolation
- Missing/duplicate trials

## 7. What EVO-001 will not do

- Call paid models
- Start native agents
- Change thresholds after seeing results (there is no live optimization run here)
- Infer approval from key availability
- Start EVO-002, DEC-040, or DEC-041
