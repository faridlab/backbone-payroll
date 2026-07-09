-- Down: drop payroll.salary_components table
DROP TABLE IF EXISTS payroll.salary_components CASCADE;
DROP FUNCTION IF EXISTS payroll.salary_components_audit_timestamp() CASCADE;
