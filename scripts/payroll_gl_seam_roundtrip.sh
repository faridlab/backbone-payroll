#!/usr/bin/env bash
# §5 round-trip: prove the payroll run engine + GL producer + events survive a full codegen regen. The
# write path + GL port + events live in user-owned custom files; regen must leave them byte-identical
# and all tests green (golden net-pay math, integrity, the REAL-accounting GL seam, the REAL-HR read).
set -euo pipefail
cd "$(dirname "$0")/.."
export DATABASE_URL="${DATABASE_URL:-postgres://postgres:postgres@localhost:5433/backbone_payroll}"
SEAM=(
  src/application/service/payroll_gl.rs
  src/application/service/payroll_events.rs
  src/application/service/payroll_write_service.rs
)
before=$(shasum "${SEAM[@]}")
echo "== regenerating (--force) =="
metaphor schema schema generate --force >/dev/null
after=$(shasum "${SEAM[@]}")
if [[ "$before" != "$after" ]]; then echo "FAIL: seam files changed across regen"; diff <(echo "$before") <(echo "$after"); exit 1; fi
echo "OK: seam files byte-identical across regen"
echo "== re-running the oracle + seams =="
SQLX_OFFLINE=false cargo test --test payroll_golden_cases --test integrity_probes \
  --test payroll_gl_seam --test payroll_hr_seam 2>&1 | grep -E "test result"
echo "OK: §5 round-trip holds"
