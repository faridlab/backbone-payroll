-- Down: drop payroll.salary_slip_lines table
DROP TABLE IF EXISTS payroll.salary_slip_lines CASCADE;
DROP FUNCTION IF EXISTS payroll.salary_slip_lines_audit_timestamp() CASCADE;
