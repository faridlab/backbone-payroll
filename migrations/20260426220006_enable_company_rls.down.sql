-- Down: remove the company RLS fence for payroll module

-- Reverse the company RLS fence for payroll.payroll_entries
DROP POLICY IF EXISTS payroll_entries_company_isolation ON payroll.payroll_entries;
ALTER TABLE payroll.payroll_entries NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payroll.payroll_entries DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for payroll.salary_slips
DROP POLICY IF EXISTS salary_slips_company_isolation ON payroll.salary_slips;
ALTER TABLE payroll.salary_slips NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payroll.salary_slips DISABLE ROW LEVEL SECURITY;

-- Reverse the company RLS fence for payroll.salary_structures
DROP POLICY IF EXISTS salary_structures_company_isolation ON payroll.salary_structures;
ALTER TABLE payroll.salary_structures NO FORCE ROW LEVEL SECURITY;
ALTER TABLE payroll.salary_structures DISABLE ROW LEVEL SECURITY;

