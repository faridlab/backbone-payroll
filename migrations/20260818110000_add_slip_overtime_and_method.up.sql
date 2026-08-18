-- Append-only columns for salary_slips: the overtime hours consumed while building the slip
-- (auditable snapshot — the pay itself is an earning line like any other) and the PPh-21 path
-- dispatched for it (progressive brackets vs an average-effective-rate category), snapshotted
-- so a later change to the employee's profile or the law tables never rewrites history.
ALTER TABLE payroll.salary_slips
  ADD COLUMN IF NOT EXISTS overtime_hours NUMERIC(8, 2) CHECK (overtime_hours IS NULL OR overtime_hours >= 0),
  ADD COLUMN IF NOT EXISTS tax_method VARCHAR(20);
