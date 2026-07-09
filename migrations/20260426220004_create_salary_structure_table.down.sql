-- Down: drop payroll.salary_structures table
DROP TABLE IF EXISTS payroll.salary_structures CASCADE;
DROP FUNCTION IF EXISTS payroll.salary_structures_audit_timestamp() CASCADE;
