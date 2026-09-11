-- Hand-authored (user-owned). Not regenerated.
--
-- Strip every company-fence artifact from the payroll tables (ADR-0029): the module is
-- tenant-agnostic; org scoping is installed by the COMPOSING service's tenancy decorator,
-- never by the module. Dropped here, per table: the company-leading indexes, the
-- <table>_company_isolation RLS policy, and the company_id column itself. Slip lines and
-- structure components carry only the denormalized fence copy of the column — their
-- org scoping rides on the parent row, so nothing else is re-based for them.
--
-- Ordering guard (the decorator must run FIRST on any database with data): the module
-- never moves tenancy data. A table is safe to strip when EITHER
--   a) it carries org_unit_id with no NULLs — the decorator backfilled it from company_id —
--      or b) it is empty (a fresh database: the earlier chain files created it empty).
-- Otherwise the strip RAISEs, naming the decorator step, rather than dropping a column
-- that still holds the only tenancy key. The file is re-runnable (every drop is IF EXISTS
-- and the tracker has no checksums), so a failed run retries cleanly after the decorator
-- lands.
--
-- RLS enable/force flags are deliberately NOT touched: the decorator owns those now.
--
-- No company-leading unique is re-based tenant-free here. The one-run-per-company
-- period unique on payroll_entries is tenancy POSTURE — a module-level
-- (period_year, period_month) unique would forbid two units of one tenant from running
-- payroll in the same month — so it is re-declared org-scoped by the composing
-- service's tenancy decorator. The slip unique (payroll_entry_id, employee_id) already
-- needs no tenant column (a run's id is a globally unique UUID) and is kept untouched.

DO $$
DECLARE
    t text;
    has_org boolean;
    org_nulls bigint;
    total bigint;
    offenders text := '';
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'payroll_entries', 'salary_slips', 'salary_slip_lines',
        'salary_structures', 'salary_components', 'compensation_changes'
    ]
    LOOP
        IF to_regclass(format('payroll.%I', t)) IS NULL THEN
            CONTINUE; -- chain not fully applied on this database; nothing to strip
        END IF;

        SELECT EXISTS (
                   SELECT 1 FROM information_schema.columns
                   WHERE table_schema = 'payroll' AND table_name = t AND column_name = 'org_unit_id'
               )
        INTO has_org;

        EXECUTE format('SELECT count(*) FROM payroll.%I', t) INTO total;

        IF has_org THEN
            EXECUTE format(
                'SELECT count(*) FROM payroll.%I WHERE org_unit_id IS NULL', t)
            INTO org_nulls;
        ELSE
            org_nulls := total; -- no org column: every row's only tenancy key is company_id
        END IF;

        IF has_org AND org_nulls = 0 THEN
            CONTINUE; -- decorator backfilled: safe
        END IF;
        IF total = 0 THEN
            CONTINUE; -- empty table (fresh database): safe
        END IF;
        offenders := offenders || format(' payroll.%s (%s rows, %s rows not covered by org_unit_id);', t, total, org_nulls);
    END LOOP;

    IF offenders <> '' THEN
        RAISE EXCEPTION 'refusing to strip company_id — these tables are not yet covered by the tenancy decorator:%. Apply the composing service''s tenancy decorator (it backfills org_unit_id from company_id) and re-run; it is the only step that moves tenancy data.', offenders;
    END IF;
END $$;

-- ── payroll_entries ────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS payroll.idx_payroll_entries_company_id_period_year_period_month;
DROP INDEX IF EXISTS payroll.idx_payroll_entries_company_id_status;
DROP POLICY IF EXISTS payroll_entries_company_isolation ON payroll.payroll_entries;
ALTER TABLE payroll.payroll_entries DROP COLUMN IF EXISTS company_id;

-- ── salary_slips ───────────────────────────────────────────────────────────────
DROP INDEX IF EXISTS payroll.idx_salary_slips_company_id_employee_id;
DROP POLICY IF EXISTS salary_slips_company_isolation ON payroll.salary_slips;
ALTER TABLE payroll.salary_slips DROP COLUMN IF EXISTS company_id;
-- The one-slip-per-employee-per-run unique (idx_salary_slips_payroll_entry_id_employee_id)
-- is a domain invariant over globally unique UUIDs and stays untouched.

-- ── salary_slip_lines ──────────────────────────────────────────────────────────
DROP INDEX IF EXISTS payroll.idx_salary_slip_lines_company_id;
DROP POLICY IF EXISTS salary_slip_lines_company_isolation ON payroll.salary_slip_lines;
ALTER TABLE payroll.salary_slip_lines DROP COLUMN IF EXISTS company_id;

-- ── salary_structures ──────────────────────────────────────────────────────────
DROP INDEX IF EXISTS payroll.idx_salary_structures_company_id_status;
-- Legacy name from before the status-lifecycle migration; already gone on any database
-- that applied the full chain. Defensive: keeps this strip re-runnable on drifted trees.
DROP INDEX IF EXISTS payroll.idx_salary_structures_company_id_is_active;
DROP POLICY IF EXISTS salary_structures_company_isolation ON payroll.salary_structures;
ALTER TABLE payroll.salary_structures DROP COLUMN IF EXISTS company_id;

-- ── salary_components ──────────────────────────────────────────────────────────
DROP INDEX IF EXISTS payroll.idx_salary_components_company_id;
DROP POLICY IF EXISTS salary_components_company_isolation ON payroll.salary_components;
ALTER TABLE payroll.salary_components DROP COLUMN IF EXISTS company_id;

-- ── compensation_changes ───────────────────────────────────────────────────────
DROP INDEX IF EXISTS payroll.idx_compensation_changes_company_id;
DROP POLICY IF EXISTS compensation_changes_company_isolation ON payroll.compensation_changes;
ALTER TABLE payroll.compensation_changes DROP COLUMN IF EXISTS company_id;
