-- Down migration: drop compensation_changes table
DROP TRIGGER IF EXISTS compensation_changes_update_audit ON payroll.compensation_changes;
DROP TRIGGER IF EXISTS compensation_changes_insert_audit ON payroll.compensation_changes;
DROP FUNCTION IF EXISTS payroll.compensation_changes_audit_timestamp();
DROP TABLE IF EXISTS payroll.compensation_changes;
-- Leave the enum type in place (other tables may adopt it); drop manually if truly unreferenced.
