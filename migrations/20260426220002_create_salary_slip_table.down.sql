-- Down: drop payroll.salary_slips table
DROP TABLE IF EXISTS payroll.salary_slips CASCADE;
DROP FUNCTION IF EXISTS payroll.salary_slips_audit_timestamp() CASCADE;
