-- Down: drop payroll.payroll_entries table
DROP TABLE IF EXISTS payroll.payroll_entries CASCADE;
DROP FUNCTION IF EXISTS payroll.payroll_entries_audit_timestamp() CASCADE;
