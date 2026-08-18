# ADR-002 — Statutory parameters as effective-dated database rows

Status: accepted · 2026-08-18 · Tier 5b (People pillar)

## Context

The statutory math (PPh 21 — both the npwp-brackets walk and the TER flat rates — BPJS shares and
wage caps, overtime band schedules) is driven by *data* that changes by ministerial decree, on the
regulator's calendar, without a code release. Before this decision that data lived in two places:
a YAML config block on the service and `Default` impls baked into the crate — so a rate change
meant a config edit or a code release, the two sources could disagree silently, and a fresh
composition had to remember to carry the YAML forward.

## Decision

**One source of truth: five typed global tables, seeded by a data-bearing migration, resolved
as-of each payroll period.**

1. **Typed tables, not one key-value blob.** `pph21_brackets`, `pph21_ptkp`, `pph21_ter_rates`,
   `bpjs_params`, `overtime_params` — each carries the shape its consumer needs (bracket bounds
   and rates; tier → annual amount; category + band; component/side/rate/wage cap; day-kind +
   hour-from/to + multiplier). A wrong-typed correction is a rejected INSERT, not a silent
   mis-parse.
2. **Every row is `effective_from`-dated.** The loader resolves each table `as-of` the first day
   of the run's period (`DISTINCT ON … ORDER BY effective_from DESC`): the newest row set at or
   before the period wins. A decree taking effect next month is a **new row** — history is never
   edited, and a past run re-reads the parameters that priced it.
3. **The loader refuses incomplete sets.** Any table with no row effective as-of the period →
   `NoParamsForPeriod` → HTTP 422 `no_statutory_params_for_period`. The tempting alternative —
   defaulting missing data to zero or to crate defaults — silently prices a run at zero tax; the
   refusal turns broken data into a loud, attributable failure. This deliberately also catches a
   lone future-dated correction row landing before its base set.
4. **No CRUD surface.** These are national-law masters, not tenant data: no HTTP writes, no
   generated entity, no company scoping. A correction is a reviewed migration (or a DBA insert
   with the same discipline), which is the right amount of ceremony for "the tax rate changed".
5. **Seeds are non-authoritative.** The migration seeds best-effort transcriptions of the cited
   regulations so a fresh composition computes real-shaped numbers. They await a statutory review;
   a confirmed correction lands as a new `effective_from` row, and the golden cases
   ([golden-cases.md](../golden-cases.md)) say which seeded rows their pins trace to.
6. **`Default`/YAML demoted to seed material.** The crate's `Default` impls and the YAML config
   block still exist for standalone/test use, but they are *where the seed values were sourced
   from*, not a second runtime source. The as-of repository is the only runtime path.

## Consequences

- A rate change never ships code: insert a dated row, next period's runs pick it up, prior runs
  are untouched.
- Compositions must run the payroll migrations to have params at all (previously the crate
  defaults papered over a missing config) — a missing table is a boot-time/migration-time fact,
  not a runtime surprise.
- Cross-country: the tables key on `country_code` (currently only `ID`); adding a country is
  adding rows, not columns.
- The per-slip compute does one extra read (the as-of resolve) per run-slip batch — negligible
  next to the slip writes, and cacheable at the run level if it ever isn't.

## Parking lot (each with a gate)

- **Seed review** — the transcribed values need confirmation against the primary sources before
  production use; corrections land as new effective-dated rows. Gate: a statutory review pass over
  `migrations/*_statutory_params*`.
- **Employer-contribution posting** — `bpjs_params` already carries the employer side, but the run
  journal books employee deductions only (inherited from ADR-001's parking lot).
- **Rest-day / holiday overtime bands** — seeded in `overtime_params` (`day_kind='rest_day'`) but
  the compute dispatches the workday schedule; rest-day pricing needs the day-kind input wired
  from the calendar. Gate: attendance exporting a day-kind (or a holiday module).
- **Deprecating the YAML block** — once no consumer reads it, drop `from_yaml_str` and the config
  block to keep one source. Gate: a consumer audit.
