DROP INDEX IF EXISTS payroll.salary_slip_lines_source_ref_idx;
ALTER TABLE payroll.salary_slip_lines
    DROP COLUMN IF EXISTS source_ref,
    DROP COLUMN IF EXISTS source_kind;
ALTER TABLE payroll.payroll_entries
    DROP COLUMN IF EXISTS timesheet_approval_id;
