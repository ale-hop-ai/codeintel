# The measured loop

codeintel has one consumer: a language model. This document is the apparatus
that decides whether a change made the tool better *for that consumer*, and the
protocol a fleet of experimenting agents works under.

It is binding the same way [docs/plan.md](plan.md) is binding. An experiment
that cannot be judged by the scoreboard defined here is not an experiment, it is
an opinion.

---

## 1. Why the loop needs a scoreboard before it needs agents

Every milestone through M6 was gated on a **done-when** — something you run. That
worked because each milestone shipped a capability that was either present or
absent. Improvement is not like that. "The schema is clearer" and "the answer is
cheaper" are claims with a direction and no scale, and a fleet of agents
optimizing claims with no scale produces six confident PRs that cannot be
compared.

So: **no experiment starts until the metric it targets has a committed baseline.**

### What makes this measurable at all

The standard library contains its own oracle and nobody has pointed it at itself.

Six rules ship in `_exact` pairs — `calls`/`calls_exact`, `dead_export`,
`entrypoint`, `depends`, `uses_package`, `ref`. The `_exact` twin is restricted
to SCIP-resolved rows, which is the compiler's answer. The name twin is a
text-matching prediction of that answer. On a corpus with a fresh SCIP index the
pair yields precision and recall directly, with no human and no clock.

**Corrected 2026-10-20, by the first scoreboard.** This section opened with
"1004 distinct callers under `calls`, 310 confirmed by `calls_exact`, 714
name-only" — measured on codeintel's own tree with SCIP stale on 45 files, and
offered as a shape rather than a result. The shape was wrong too, and wrong in
the tool's favour. Against a fresh oracle, name-tier `calls` is **97.8 %
precise on fd and 90.3 % on tokio**.

A stale oracle does not add noise to this measurement, it adds bias: every file
SCIP has not re-indexed contributes name-only rows that count as false
positives. The lesson is the harness rule, not the number — **a scoreboard
taken against a stale oracle is void**, and the bench enforces it.

`dead_export`'s doc string says *"OVER-reports on name"*. `entrypoint`'s says
*"INVERTS on name provenance"*. Those are qualitative hedges inside a schema an
agent reads as fact. This document turns them into committed numbers.

### What the loop must be built on

The benchmark host is a 10-core M5 under a heavy EDR tax, and the committed
`benches/corpora/*/baseline.json` files were taken on different hardware. **Wall
clock therefore cannot be the inner gate.**

This is not a limitation. The two properties that decide whether codeintel pays
for itself — **what an answer costs in tokens** and **whether an answer is
true** — are both exactly deterministic and machine-independent. Latency stays a
Tier-2 advisory, measured where `benches/corpus.rs` already measures it.

---

## 2. The four metric families

Everything the scoreboard reports falls in one of four families. The families
are deliberately **opposed in pairs**: cost cannot be won by truncating, because
truncation surfaces as recall loss; precision cannot be won by emitting less,
because coverage holds the floor.

### Cost — what the answer spends

| Metric | Definition |
|---|---|
| `schema_bytes` | bytes of the `{"schema":true}` response, whole MCP envelope |
| `answer_bytes_p50` / `_p95` / `_total` | bytes of the whole MCP tool result, over the corpus query set |
| `bytes_per_row` | `answer_bytes_total` / rows returned |

Bytes, not tokens. Bytes are exact and tokenizer-independent; the ratio to
tokens is stable enough that a 3x byte cut is a 3x token cut.

### Precision — whether the answer is true

For each rule with an `_exact` twin, over a corpus whose SCIP index is **fresh**
(`status: "ok"`, never `scip-stale` — a stale index inflates name-only rows and
makes the metric lie):

```
tp = rows in both        fp = rows in name only       fn = rows in exact only
precision = tp / (tp + fp)        recall = tp / (tp + fn)
```

Reported per rule, never averaged into a single score. Averaging provenance is
invariant 2's exact prohibition, and averaging precision across rules is the
same mistake one level up.

### Coverage — what the extractor saw

| Metric | Today, on this tree |
|---|---|
| `anchor_rate` | 95.2% |
| `span_coverage` (`def_span` / `def`, distinct symbols) | 0.37 fd / 0.35 tokio |
| `role_known` (`scip_ref` rows with `Role != "unknown"`) | **0%** |
| `scip_impl_rows` | **0** |

Two of those are zero. `Role` being entirely `unknown` means `is_test`'s SCIP
clause is inert and the read/write/import vocabulary is advertised with no data
behind it.

### Calibration — whether the tool knows what it does not know

codeintel does not promise to be right. It promises to say which tier answered,
so an agent knows when to distrust it. That promise has never been measured.

```
calibration = (oracle-wrong rows carrying Prov="name" or status != "ok")
            / (oracle-wrong rows)
```

A perfect 1.0 means every wrong answer was flagged. **This is the metric to
refuse to regress on**, because it is the one that makes invariant 2 worth
having. A change that raises precision while lowering calibration has made the
tool more confidently wrong, and that is worse than leaving it alone.

**What the first scoreboard found, and it is worse than a low score.** fd reads
0.50 and tokio 0.4944 — and *every flagged row is a `ref` row*. `calls`,
`dead_export`, `entrypoint`, `depends` and `uses_package` carry no `Prov`
column in their own tuples, so a wrong row from any of them is **structurally
unflaggable**. The "or `status != ok`" clause is inert by construction, because
the harness refuses to measure a corpus whose status is not `ok`.

So this metric today reports one thing: the share of wrong rows that came from
`ref`. And the claim above it — that codeintel "promises to say which tier
answered" — is **false at the row level for five of the six rules**. The promise
holds for the relation an agent reads through `ref/7`, and nowhere else.

That is a finding about the tool, not about the metric, and it outranks every
cost hypothesis in § 7: a cheaper answer that is still silently wrong is a worse
tool, not a better one. Until a rule's output can carry its own provenance, the
honest reading of this number is **"undefined for five of six rules"**, and it
may not be averaged into a headline.

**Closed by `tier/4` (exp/prov-rows).** One added relation hands `Prov` back to
the five rules without changing any head (`specs/01-facts.md` § Row
provenance); calibration reads 1.0 on fd and tokio, with every precision, recall
and answer byte unchanged. Read the 1.0 for what it is: the oracle is the
`_exact` twin and `tier`'s `exact` is defined as "the twin also yields it", so a
row the oracle calls wrong is `name` by construction. The metric now verifies
that the promise is *kept*, not that `name` is *calibrated* — a `name` row that
is right still counts as an honest guess. The promise holds only for an agent
that asks `tier`; the bare heads are unchanged.

---

## 3. The scoreboard artifact

`target/score/<corpus>.json`. Same rev-pinning and refuse-to-compare discipline
as `benches/corpora/*/baseline.json`: a baseline records the rev it measured,
and a comparison against a different tree is an error, not a warning.

```json
{
  "corpus":  { "name": "fd", "rev": "b422e5d...", "files": 24, "defs": 638 },
  "scip":    { "status": "ok", "anchor_rate": 0.952 },
  "cost":    { "schema_bytes": 18304, "answer_bytes_p50": 0,
               "answer_bytes_p95": 0, "answer_bytes_total": 0,
               "bytes_per_row": 0.0 },
  "precision": {
    "calls":        { "tp": 0, "fp": 0, "fn": 0, "precision": 0.0, "recall": 0.0 },
    "dead_export":  { "tp": 0, "fp": 0, "fn": 0, "precision": 0.0, "recall": 0.0 },
    "entrypoint":   { "tp": 0, "fp": 0, "fn": 0, "precision": 0.0, "recall": 0.0 },
    "depends":      { "tp": 0, "fp": 0, "fn": 0, "precision": 0.0, "recall": 0.0 },
    "uses_package": { "tp": 0, "fp": 0, "fn": 0, "precision": 0.0, "recall": 0.0 },
    "ref":          { "tp": 0, "fp": 0, "fn": 0, "precision": 0.0, "recall": 0.0 }
  },
  "coverage":    { "span_coverage": 0.0, "role_known": 0.0, "scip_impl_rows": 0 },
  "calibration": { "flagged": 0, "wrong": 0, "calibration": 0.0 }
}
```

**`scip.status` is load-bearing.** A scoreboard taken against a `scip-stale`
corpus is void and the harness must refuse to write it. Invariant 5 applies to
the measurement apparatus exactly as it applies to the tool.

---

## 4. Four tiers

| Tier | Command | Budget | Role |
|---|---|---|---|
| 0 | `cargo test -p codeintel --test counters --test cost` | seconds | every edit |
| 1 | `provision apply tasks/score.yml` — fd only | ~1 min | **the experiment gate** |
| 2 | Tier 1 on tokio + `tasks/bench.yml` | ~10 min | pre-merge |
| 3 | the agent eval, automated | hours | merge gate for schema, rule signatures, rendering |

Tier 0's `cost.rs` is `counters.rs`'s twin: it pins **answer bytes** per fixture
query the way `counters.rs` pins `derived`. Exact, clock-free, CI-safe, and
`UPDATE_GOLDEN=1` carries the same obligation — read the diff and say which
kind of change it was before you refresh it.

If an experiment cannot be judged at Tier 1, nobody iterates. One minute is the
design target, not an aspiration.

---

## 5. The eval bank, split three ways

[docs/agent-eval.md](agent-eval.md) is right that re-running after every copy
change is how a score stops measuring the tool and starts measuring how many
times you re-rolled. That rule is kept, by splitting the bank rather than
weakening it.

| Bank | Size | Re-run | Purpose |
|---|---|---|---|
| **Frozen** | the current 30 (sets A/B) | only on a signature change | rot detection, guarded by `tests/eval_rot.rs` |
| **Working** | 60-100, new | freely, every Tier 3 | the iteration signal |
| **Held-out** | ~30, new | merge only | the anti-overfit tripwire |

The working set's reference answers are **recomputed from their reference
queries on every run**. That is this eval's own hardest-won lesson, stated in
its own words: *a reference answer nobody recomputes is an assertion nobody
checks.* Three reference-answer defects have been found that way and every one
was invisible until something forced a recomputation.

**An experimenting agent never sees the held-out set.** If it did, the set would
measure memorization and the number would be worthless.

---

## 6. Fleet protocol

### What an agent gets

- its own pad, and its own worktree at `~/worktrees/codeintel/exp-<slug>`
- **one hypothesis naming one target metric**, written down before it starts
- the committed baseline for that metric

### What an agent may not touch

- `benches/corpora/*/score-baseline.json` — moving the baseline to pass the gate
  is the only way to cheat this loop, so it is the one edit that is forbidden
  outright
- the frozen eval bank and `tests/eval_rot.rs`
- the held-out bank
- the lint block in the root `Cargo.toml` (drift-checked; change it upstream)

### The gate — automated, never self-assessed

1. `provision apply tasks/ci.yml` green
2. no scoreboard metric regresses
3. at least one target metric improves
4. the surface budget is unchanged — 5 CLI verbs, 1 MCP tool — unless the
   hypothesis says otherwise *and that was signed off before the work started*

An agent reporting its own verdict is not a gate. The scoreboard diff is.

### Reporting

An experiment returns its branch, its scoreboard diff, and a one-line verdict.
A hypothesis that was **disproved is a result, not a failure** — it is written
up and the branch is kept. The loop learns from refutations or it is not a loop.

---

## 7. The standing hypotheses

Opened by the 2026-10-20 audit. Each is independently testable against a metric
defined above.

| # | Hypothesis | Target |
|---|---|---|
| 1 | Dropping `content[0].text` when the client takes `structuredContent`, and `rows` behind `raw:true`, cuts answer bytes ~3x with no capability loss | `answer_bytes`, eval flat |
| 2 | `schema_bytes` is three copies of one catalog, not a bloated catalog | `schema_bytes`, eval |
| 3 | Wiring SCIP roles makes `is_test` and read/write real | `role_known`, `is_test` P/R |
| 4 | `schema` and `status` refreshing like `query` removes the catalog/reality mismatch | coverage consistency |
| 5 | Publishing per-rule precision *in the schema* improves an agent's choice of `_exact` variants | working-set eval, calibration |
| 6 | Tier-A name matching gains recall without losing precision — method calls `x.f()` are mostly missing | `calls` P/R |

**Hypothesis 2 was restated by the first scoreboard.** It read "the schema fits
in <6 KB, from 18.3 KB" and assumed a diet was needed. Measured: 15,407 bytes on
fd, of which `content[0].text` is 5,189 and `structuredContent` is 10,036 — and
`structuredContent.text` is a **byte-identical copy** of `content[0].text`. The
catalog is already under 6 KB. The surplus is three copies of it, so hypothesis
2 is the same deduplication as hypothesis 1 and not a separate editing job.

Hypothesis 5 is the one that closes the loop: it feeds the scoreboard back into
the surface the agent reads, so the tool's own honesty becomes something it
tells you rather than something this document records.

---

## 8. What this document does not authorize

- **No scores.** Invariant 4 is unchanged. Precision and recall are measurements
  of *the tool*, against an oracle, on a pinned corpus. They are not rankings of
  *code*, and no metric here may be joined onto a symbol and reported to an
  agent as a quality number.
- **No new CLI verbs.** The scoreboard is a bench, like `corpus.rs`, run by a
  task. The budget is 5 and 1. An improvement plan that begins by growing the
  surface has already lost the argument the project exists to make.
- **No averaging across provenance.** Per-rule, always.
