# Assessment counterexample corpus and compare budgets (DEC-013)

This page freezes the **pre-registered** offline evaluation inputs for
model-free engineering assessment. It reuses the EVO-001 pre-registration
style (freeze corpus identity, expect/forbid, metrics, and budgets **before**
the first candidate run) but **counts separately** from EVO. It does **not**
require EVO paid/dual-host experiments first, and it does **not** build a
second evaluation platform.

Machine companions:

- Corpus: [`tests/fixtures/assessment/counterexamples/`](../../tests/fixtures/assessment/counterexamples/)
- Compare harness: [`tests/benchmarks/assessment/`](../../tests/benchmarks/assessment/)
- Reused contracts: `tests/fixtures/assessment/{contracts,signals,envelope}/`

## 1. Acceptance mapping

| # | Acceptance (Chinese) | Where frozen |
| --- | --- | --- |
| 1 | 冻结覆盖矩阵、fixture 身份、预期/禁用行为及比较脚本；首次候选运行前固定性能预算和统计口径。 | `counterexamples/coverage-matrix.json`, per-fixture `expect`/`forbidden`, `benchmarks/assessment/{contract,budgets,compare}.py` |
| 2 | 确定性结构正确性、运行开销、建议有效性分开；模型成功率、节约金额和语义理解能力未测不推导。 | contract `metric_families` + `unmeasured_not_derived`; compare refuses merged scores / derived fields |
| 3 | 涵盖错误重试、撤权、错任务、证据撤回、缺字段、投递未知和时间预算；关键硬约束失败不以平均数抵消。 | critical families on C02/C03/C04/C08/C12/C14/C15; compare rejects `claimed_pass_via_average` |

## 2. Corpus (C01–C20 + boundary positives)

Twenty synthetic counterexample classes from ACCEPTANCE.md, plus five positive
boundary fixtures (P01–P05). Each fixture declares:

- stable `case` id and `slug` (fixture identity)
- `expect` and `forbidden` behaviors
- optional `hard_constraint` + `critical_family` (zero tolerance)

Critical families (violations must not be offset by averages):

- `error_retry` (C03)
- `revoke` (C08)
- `wrong_task` (C02)
- `evidence_withdrawal` (C14)
- `missing_fields` (C04)
- `delivery_unknown` (C15)
- `time_budget` (C12)

Level is **L1 synthetic**. This corpus does not substitute L4 real-business
acceptance.

## 3. Baseline vs candidate

| Arm | Setting | Notes |
| --- | --- | --- |
| Baseline | same source SHA + fixture + policy + as_of; `assessment_explain=off` | Existing tip behavior without new explain |
| Candidate | same identity; `assessment_explain=on`; **read-only** | No model/network; no claim/completion side effects |

Recorded cost dimensions (separate): `collect`, `judge`, `output`.

## 4. Metric families (kept separate)

1. **structural_correctness** — deterministic fixture expect/forbid / hard gates
2. **runtime_overhead** — collect/judge/output latency & bytes vs frozen budgets
3. **advisory_effectiveness** — advisory code/reason match on fixtures only

**Not derived** from unmeasured data:

- model success rate
- dollar savings
- semantic understanding

## 5. Statistics 口径 (frozen before first candidate)

- Percentile: nearest-rank
- p95: sort ascending; index = ceil(0.95×N)−1
- Require **N ≥ 30** before labeling p95 (single samples must not be called p95)
- Timeouts, failures, and overruns stay in the denominator
- Cold and warm reported separately; warmups do not count as samples

## 6. Performance budgets

`tests/benchmarks/assessment/budgets.json` freezes **slots and rules** before the
first candidate run. Absolute millisecond ceilings are bound from a measured
baseline (`status`: `frozen_slots_pending_baseline_binding` →
`frozen_with_baseline`). This page does **not** invent fixed millisecond gains.

Hard rules:

- candidate tool-call delta default max = 0 vs baseline
- output return-bytes absolute ceiling = 262144 until revised with evidence
- hard-constraint failures fail the compare even if soft averages look good
- `compare.py` does not label a pair passed while budget status is still
  `frozen_slots_pending_baseline_binding`
- a missing cost field stays missing; it is not treated as zero and cannot
  look like a millisecond improvement

## 7. How to verify (offline)

```sh
python3 tests/fixtures/assessment/counterexamples/verify.py
python3 tests/benchmarks/assessment/verify.py
python3 -m unittest tests.benchmarks.assessment.test_compare
python3 tests/fixtures/assessment/envelope/verify.py
python3 tests/fixtures/assessment/signals/verify.py
python3 tests/fixtures/assessment/contracts/verify.py
```

Compare two receipts:

```sh
python3 tests/benchmarks/assessment/compare.py \
  --baseline path/to/baseline_receipt.json \
  --candidate path/to/candidate_receipt.json
```

## 8. Boundaries

- Assessment remains read-only; no model or network path
- Missing fields stay missing (never zero-filled); unknown ≠ false
- Reuses DEC-010/011/012 AssessmentEnvelope + FactSnapshot; no second eval stack
- EVO-000 / paid dual-host work is **not** started by this card
