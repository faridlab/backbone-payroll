-- Migration: replace the salary-structure lifecycle boolean with a status enum
-- salary_structures carried `is_active BOOLEAN NOT NULL DEFAULT TRUE`; the
-- tree-wide convention is one `status` enum field per lifecycle (see
-- docs/refactoring-schema in the serpa workspace). The boolean migrates only
-- rows deviating from its own column default. The enum type is created
-- unqualified so it lands beside the module's other enum types (public), where
-- the generated sqlx type_name resolves.

DO $$ BEGIN
    CREATE TYPE salary_structure_status AS ENUM ('active', 'inactive');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

ALTER TABLE payroll.salary_structures ADD COLUMN status salary_structure_status NOT NULL DEFAULT 'active';
UPDATE payroll.salary_structures SET status = 'inactive' WHERE NOT is_active;
ALTER TABLE payroll.salary_structures DROP COLUMN is_active;

DROP INDEX IF EXISTS payroll.idx_salary_structures_company_id_is_active;
CREATE INDEX idx_salary_structures_company_id_status ON payroll.salary_structures (company_id, status);
