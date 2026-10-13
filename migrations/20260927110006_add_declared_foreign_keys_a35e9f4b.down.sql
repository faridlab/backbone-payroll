-- Down: drop the declared foreign keys for payroll module
ALTER TABLE IF EXISTS payroll.salary_component_versions DROP CONSTRAINT IF EXISTS fk_salary_component_versions_component_id;
ALTER TABLE IF EXISTS payroll.salary_components DROP CONSTRAINT IF EXISTS fk_salary_components_structure_id;
ALTER TABLE IF EXISTS payroll.salary_slip_lines DROP CONSTRAINT IF EXISTS fk_salary_slip_lines_salary_slip_id;
ALTER TABLE IF EXISTS payroll.salary_slips DROP CONSTRAINT IF EXISTS fk_salary_slips_payroll_entry_id;
