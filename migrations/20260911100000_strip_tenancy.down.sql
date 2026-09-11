-- Hand-authored (user-owned). Not regenerated.
--
-- Best-effort restore sketch for the tenancy strip (ADR-0029). This is a breaking module
-- release against dev-stage databases: the down re-adds the company_id column as nullable
-- with the company-leading indexes in their final pre-strip shapes, but restores NO data —
-- rows written after the strip (or after the decorator re-keyed them) carry org_unit_id
-- only. The composing service's tenancy decorator remains the live fence; the
-- <table>_company_isolation policies are NOT recreated here. Treat this down as a
-- schema-shape sketch for archaeology, not a usable rollback.

ALTER TABLE payroll.payroll_entries        ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE payroll.salary_slips           ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE payroll.salary_slip_lines      ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE payroll.salary_structures      ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE payroll.salary_components      ADD COLUMN IF NOT EXISTS company_id uuid;
ALTER TABLE payroll.compensation_changes   ADD COLUMN IF NOT EXISTS company_id uuid;

-- ── payroll_entries ────────────────────────────────────────────────────────────
CREATE UNIQUE INDEX IF NOT EXISTS idx_payroll_entries_company_id_period_year_period_month
    ON payroll.payroll_entries (company_id, period_year, period_month);
CREATE INDEX IF NOT EXISTS idx_payroll_entries_company_id_status
    ON payroll.payroll_entries (company_id, status);

-- ── salary_slips ───────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_salary_slips_company_id_employee_id
    ON payroll.salary_slips (company_id, employee_id);

-- ── salary_slip_lines ──────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_salary_slip_lines_company_id
    ON payroll.salary_slip_lines (company_id);

-- ── salary_structures ──────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_salary_structures_company_id_status
    ON payroll.salary_structures (company_id, status);

-- ── salary_components ──────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_salary_components_company_id
    ON payroll.salary_components (company_id);

-- ── compensation_changes ───────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_compensation_changes_company_id
    ON payroll.compensation_changes (company_id);
