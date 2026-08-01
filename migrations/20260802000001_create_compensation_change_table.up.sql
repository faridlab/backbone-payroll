-- Migration: Create compensation_changes table
-- ADR-005: the payroll-side receiver for the lifecycle compound events. A `promotion.effective`
-- appends a row (change_type='promotion', new_amount=proposed_salary); an `offboarding.closed`
-- appends a row (change_type='adjustment', a placeholder settlement amount). `reference_id` is the
-- non-null idempotency link back to the source workflow (promotion_id / offboarding_id) — set for
-- every row written by an event so re-delivery is detectable.
--
-- Generated-style DDL (hand-written to match the codegen conventions; the matching
-- schema/models/compensation_change.model.yaml is the SSoT for any future CRUD codegen).

-- Create compensation_change_type enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'compensation_change_type') THEN
        CREATE TYPE compensation_change_type AS ENUM ('hire', 'promotion', 'transfer', 'adjustment', 'offboarding');
    END IF;
END
$$;

CREATE SCHEMA IF NOT EXISTS payroll;

CREATE TABLE IF NOT EXISTS payroll.compensation_changes (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    company_id UUID NOT NULL,
    employee_id UUID NOT NULL,
    change_type compensation_change_type NOT NULL,
    new_amount NUMERIC(18, 2) CHECK (new_amount IS NULL OR new_amount >= 0),
    effective_date DATE,
    reference_id UUID,
    note TEXT,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_compensation_changes_company_id ON payroll.compensation_changes (company_id);

CREATE INDEX IF NOT EXISTS idx_compensation_changes_employee_id ON payroll.compensation_changes (employee_id);

-- The idempotency link: one row per source workflow event.
CREATE INDEX IF NOT EXISTS idx_compensation_changes_reference_id ON payroll.compensation_changes (reference_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_compensation_changes_metadata_gin ON payroll.compensation_changes USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_compensation_changes_metadata_deleted_at ON payroll.compensation_changes ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_compensation_changes_metadata_created_at ON payroll.compensation_changes ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_compensation_changes_metadata_updated_at ON payroll.compensation_changes ((metadata->>'updated_at'));

-- Triggers for automatic metadata timestamp management
CREATE OR REPLACE FUNCTION payroll.compensation_changes_audit_timestamp() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{created_at}', to_jsonb(NOW()));
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    ELSIF TG_OP = 'UPDATE' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS compensation_changes_insert_audit ON payroll.compensation_changes;
CREATE TRIGGER compensation_changes_insert_audit BEFORE INSERT ON payroll.compensation_changes
    FOR EACH ROW EXECUTE FUNCTION payroll.compensation_changes_audit_timestamp();

DROP TRIGGER IF EXISTS compensation_changes_update_audit ON payroll.compensation_changes;
CREATE TRIGGER compensation_changes_update_audit BEFORE UPDATE ON payroll.compensation_changes
    FOR EACH ROW EXECUTE FUNCTION payroll.compensation_changes_audit_timestamp();
