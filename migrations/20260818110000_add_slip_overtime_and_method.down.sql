-- Inverse of 20260818110000_add_slip_overtime_and_method.up.sql.
-- Dropping these columns erases the audit snapshot of how each slip was computed; only run
-- this when decommissioning the snapshot columns entirely.
ALTER TABLE payroll.salary_slips
  DROP COLUMN IF EXISTS tax_method,
  DROP COLUMN IF EXISTS overtime_hours;
