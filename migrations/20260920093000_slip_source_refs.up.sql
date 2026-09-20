-- Provenance for the payroll handoff: what produced a slip line, and which
-- approved timesheet period a run consumed. Nullable by design — legacy rows
-- and structure-computed lines carry no source.

ALTER TABLE payroll.salary_slip_lines
    ADD COLUMN IF NOT EXISTS source_kind text,
    ADD COLUMN IF NOT EXISTS source_ref uuid;

ALTER TABLE payroll.payroll_entries
    ADD COLUMN IF NOT EXISTS timesheet_approval_id uuid;

CREATE INDEX IF NOT EXISTS salary_slip_lines_source_ref_idx
    ON payroll.salary_slip_lines (source_ref) WHERE source_ref IS NOT NULL;
