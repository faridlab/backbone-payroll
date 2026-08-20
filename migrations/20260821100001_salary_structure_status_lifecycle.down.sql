-- Down: restore the is_active boolean and the composite index keyed on it.
-- Only 'inactive' rows are written back as FALSE; rows at the column default
-- map to the boolean default TRUE without an UPDATE.

DROP INDEX IF EXISTS payroll.idx_salary_structures_company_id_status;

ALTER TABLE payroll.salary_structures ADD COLUMN is_active BOOLEAN NOT NULL DEFAULT TRUE;
UPDATE payroll.salary_structures SET is_active = FALSE WHERE status = 'inactive';
ALTER TABLE payroll.salary_structures DROP COLUMN status;
DROP TYPE IF EXISTS salary_structure_status;

CREATE INDEX idx_salary_structures_company_id_is_active ON payroll.salary_structures (company_id, is_active);
