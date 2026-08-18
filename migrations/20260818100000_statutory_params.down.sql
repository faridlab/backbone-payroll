-- Inverse of 20260818100000_statutory_params.up.sql.
-- Drops the statutory parameter tables. Any deployment that has already run payroll against
-- these rows loses its parameter history — only run this when decommissioning the module's
-- statutory resolution (slip building will then fail closed until rows are re-seeded).
DROP TABLE IF EXISTS payroll.overtime_params;
DROP TABLE IF EXISTS payroll.bpjs_params;
DROP TABLE IF EXISTS payroll.pph21_ter_rates;
DROP TABLE IF EXISTS payroll.pph21_ptkp;
DROP TABLE IF EXISTS payroll.pph21_brackets;
