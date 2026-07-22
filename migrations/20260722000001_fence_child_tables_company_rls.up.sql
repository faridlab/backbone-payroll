-- Migration: company row-level-security fence for payroll child tables (ADR-0010 Decision A)
--
-- Extends the ADR-0008 fence to the two remaining child tables — salary_slip_lines and
-- salary_components — by adding a DENORMALIZED company_id column (sourced FROM PARENT) and the
-- same FORCE RLS + company_isolation policy the parents already carry. Parents are already fenced
-- so the backfill is deterministic and no fail-loud guard is required.
--
-- Pattern matches backbone-billing / backbone-inventory / backbone-manufacturing child fences.

-- =============================================================================
-- salary_slip_lines ← salary_slips (FK: salary_slip_lines.salary_slip_id → salary_slips.id)
-- =============================================================================
ALTER TABLE payroll.salary_slip_lines ADD COLUMN IF NOT EXISTS company_id UUID;

UPDATE payroll.salary_slip_lines AS l
   SET company_id = s.company_id
  FROM payroll.salary_slips AS s
 WHERE l.company_id IS NULL
   AND s.id = l.salary_slip_id;

ALTER TABLE payroll.salary_slip_lines ALTER COLUMN company_id SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_salary_slip_lines_company_id
    ON payroll.salary_slip_lines (company_id);

ALTER TABLE payroll.salary_slip_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE payroll.salary_slip_lines FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS salary_slip_lines_company_isolation ON payroll.salary_slip_lines;
CREATE POLICY salary_slip_lines_company_isolation ON payroll.salary_slip_lines
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);

-- =============================================================================
-- salary_components ← salary_structures (FK: salary_components.structure_id → salary_structures.id)
-- =============================================================================
ALTER TABLE payroll.salary_components ADD COLUMN IF NOT EXISTS company_id UUID;

UPDATE payroll.salary_components AS c
   SET company_id = s.company_id
  FROM payroll.salary_structures AS s
 WHERE c.company_id IS NULL
   AND s.id = c.structure_id;

ALTER TABLE payroll.salary_components ALTER COLUMN company_id SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_salary_components_company_id
    ON payroll.salary_components (company_id);

ALTER TABLE payroll.salary_components ENABLE ROW LEVEL SECURITY;
ALTER TABLE payroll.salary_components FORCE  ROW LEVEL SECURITY;

DROP POLICY IF EXISTS salary_components_company_isolation ON payroll.salary_components;
CREATE POLICY salary_components_company_isolation ON payroll.salary_components
    FOR ALL
    USING      (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid)
    WITH CHECK (company_id = NULLIF(current_setting('app.company_id', true), '')::uuid);
