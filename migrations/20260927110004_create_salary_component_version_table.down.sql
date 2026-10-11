-- Down: drop payroll.salary_component_versions table
DROP TABLE IF EXISTS payroll.salary_component_versions CASCADE;
DROP FUNCTION IF EXISTS payroll.salary_component_versions_audit_timestamp() CASCADE;
