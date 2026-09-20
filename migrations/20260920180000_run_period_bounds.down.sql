ALTER TABLE payroll.payroll_entries DROP CONSTRAINT IF EXISTS payroll_entries_bounds_order;
ALTER TABLE payroll.payroll_entries
    DROP COLUMN IF EXISTS period_end,
    DROP COLUMN IF EXISTS period_start;
