-- A run whose period is NOT the calendar month (a 26th→25th cut-off is
-- common practice): explicit bounds override the calendar reading of
-- (period_year, period_month); NULL keeps the calendar month exactly as
-- before.

ALTER TABLE payroll.payroll_entries
    ADD COLUMN IF NOT EXISTS period_start date,
    ADD COLUMN IF NOT EXISTS period_end date;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'payroll_entries_bounds_order'
    ) THEN
        ALTER TABLE payroll.payroll_entries
            ADD CONSTRAINT payroll_entries_bounds_order
            CHECK (period_start IS NULL AND period_end IS NULL
                   OR period_start IS NOT NULL AND period_end IS NOT NULL
                      AND period_start <= period_end);
    END IF;
END $$;
