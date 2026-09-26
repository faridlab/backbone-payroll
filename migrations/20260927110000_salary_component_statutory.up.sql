-- Statutory marker on a salary component (owner ruling 2026-09-26): the
-- split between what the state takes (BPJS, PPh 21) and what the company
-- deducts must be visible on the STRUCTURE, before a slip is produced. The
-- slip line keeps its own copy so a run stays historically accurate when
-- the statutory rules change.
ALTER TABLE payroll.salary_components
    ADD COLUMN IF NOT EXISTS is_statutory BOOLEAN NOT NULL DEFAULT FALSE;
