-- Down: reverse the ADR-0010 child-table company RLS fence for payroll.

-- =============================================================================
-- salary_slip_lines
-- =============================================================================
DROP POLICY IF EXISTS salary_slip_lines_company_isolation ON payroll.salary_slip_lines;
ALTER TABLE payroll.salary_slip_lines NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payroll.salary_slip_lines DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS payroll.idx_salary_slip_lines_company_id;
ALTER TABLE payroll.salary_slip_lines DROP COLUMN IF EXISTS company_id;

-- =============================================================================
-- salary_components
-- =============================================================================
DROP POLICY IF EXISTS salary_components_company_isolation ON payroll.salary_components;
ALTER TABLE payroll.salary_components NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payroll.salary_components DISABLE ROW LEVEL SECURITY;

DROP INDEX IF EXISTS payroll.idx_salary_components_company_id;
ALTER TABLE payroll.salary_components DROP COLUMN IF EXISTS company_id;
