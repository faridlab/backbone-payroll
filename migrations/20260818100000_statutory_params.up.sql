-- Statutory parameter tables: national-law rate data as versioned rows, resolved as-of each
-- payroll period (the row with the greatest effective_from <= the period start applies). They
-- are global masters without company_id on purpose: the law they encode is country-wide, and
-- giving them a mutation surface (CRUD routes) would invite per-company edits of law data —
-- so they exist only here, seeded by migration, never as schema models.
--
-- The seeded values are a STARTER SET aligned with the current-law defaults that shipped in
-- config/application.yml; the TER bands are a reduced, coarse-grained representation of the
-- regulation's published table. They are NOT yet reviewed statutory data: when a domain
-- reviewer signs off (or the law changes), the correction is a NEW COMPLETE effective-dated SET
-- for the affected table — every row restated at the new effective_from, the unchanged rows
-- copied verbatim — never a lone row, never an edit to live rows, and never a code change. The
-- as-of loader resolves each table to the whole row set at the greatest effective_from <= the
-- period and refuses the period when that set is incomplete, so a lone correction row reads as
-- a broken set (a refused run) rather than as new law.
--
-- PPh-21 progressive brackets + PTKP (UU HPP, effective 2022-01-01).
CREATE TABLE payroll.pph21_brackets (
  country_code  text        NOT NULL,
  effective_from date       NOT NULL,
  seq           int         NOT NULL,
  lower_bound   numeric(20,2) NOT NULL DEFAULT 0,
  upper_bound   numeric(20,2),
  rate          numeric(7,6)  NOT NULL,
  PRIMARY KEY (country_code, effective_from, seq),
  CHECK (rate >= 0 AND rate <= 1),
  CHECK (upper_bound IS NULL OR upper_bound > lower_bound)
);

CREATE TABLE payroll.pph21_ptkp (
  country_code  text        NOT NULL,
  effective_from date       NOT NULL,
  tier          text        NOT NULL CHECK (tier IN ('tk0','tk1','tk2','tk3','k0','k1','k2','k3')),
  annual_amount numeric(20,2) NOT NULL,
  PRIMARY KEY (country_code, effective_from, tier),
  CHECK (annual_amount >= 0)
);

-- TER (Average Effective Rate) bands, keyed by monthly base (gross minus the BPJS employment-
-- insurance employee share). `lower_bound` opens the band; the next seq's lower_bound closes it.
-- Reduced starter set pending statutory review (see header).
CREATE TABLE payroll.pph21_ter_rates (
  country_code  text        NOT NULL,
  effective_from date       NOT NULL,
  category      text        NOT NULL CHECK (category IN ('ter_a','ter_b','ter_c')),
  seq           int         NOT NULL,
  lower_bound   numeric(20,2) NOT NULL DEFAULT 0,
  rate          numeric(7,6)  NOT NULL,
  PRIMARY KEY (country_code, effective_from, category, seq),
  CHECK (rate >= 0 AND rate <= 1)
);

-- BPJS component rates. wage_cap NULL = uncapped for that component. JKK risk classes land as
-- components jkk_1..jkk_5 (employer side only, uncapped).
CREATE TABLE payroll.bpjs_params (
  country_code  text        NOT NULL,
  effective_from date       NOT NULL,
  component     text        NOT NULL CHECK (component IN
    ('kes','jht','jp','jkk_1','jkk_2','jkk_3','jkk_4','jkk_5','jkm')),
  side          text        NOT NULL CHECK (side IN ('employee','employer')),
  rate          numeric(7,6)  NOT NULL,
  wage_cap      numeric(20,2),
  PRIMARY KEY (country_code, effective_from, component, side),
  CHECK (rate >= 0),
  CHECK (wage_cap IS NULL OR wage_cap >= 0)
);

-- Overtime multiplier bands over the hour sequence (hour_from..hour_to of the shift's overtime
-- hours; hour_to NULL = open-ended). Rest-day bands are seeded for completeness but are not
-- dispatched yet — workday bands are what the slip builder applies today.
CREATE TABLE payroll.overtime_params (
  country_code  text        NOT NULL,
  effective_from date       NOT NULL,
  day_kind      text        NOT NULL CHECK (day_kind IN ('workday','rest_day')),
  hour_from     int         NOT NULL,
  hour_to       int,
  multiplier    numeric(5,2)  NOT NULL,
  PRIMARY KEY (country_code, effective_from, day_kind, hour_from),
  CHECK (hour_from >= 0),
  CHECK (hour_to IS NULL OR hour_to >= hour_from),
  CHECK (multiplier >= 0)
);

-- ---------------------------------------------------------------- seeds: 'ID' --
-- PPh-21 brackets (UU HPP) — identical to the shipped YAML/default values.
INSERT INTO payroll.pph21_brackets (country_code, effective_from, seq, lower_bound, upper_bound, rate) VALUES
  ('ID','2022-01-01',1,          0,  60000000, 0.050000),
  ('ID','2022-01-01',2,  60000000, 250000000, 0.150000),
  ('ID','2022-01-01',3, 250000000, 500000000, 0.250000),
  ('ID','2022-01-01',4, 500000000,5000000000, 0.300000),
  ('ID','2022-01-01',5,5000000000,       NULL, 0.350000);

-- PTKP annual relief by tier.
INSERT INTO payroll.pph21_ptkp (country_code, effective_from, tier, annual_amount) VALUES
  ('ID','2022-01-01','tk0', 54000000),
  ('ID','2022-01-01','tk1', 58500000),
  ('ID','2022-01-01','tk2', 63000000),
  ('ID','2022-01-01','tk3', 67500000),
  ('ID','2022-01-01','k0',  58500000),
  ('ID','2022-01-01','k1',  63000000),
  ('ID','2022-01-01','k2',  67500000),
  ('ID','2022-01-01','k3',  72000000);

-- TER bands — reduced starter set (coarse bands standing in for the regulation's fine-grained
-- table; anchors chosen so commonly-seen bases land on the published headline rates).
INSERT INTO payroll.pph21_ter_rates (country_code, effective_from, category, seq, lower_bound, rate) VALUES
  ('ID','2024-01-01','ter_a', 1,         0, 0.000000),
  ('ID','2024-01-01','ter_a', 2,   5400000, 0.002500),
  ('ID','2024-01-01','ter_a', 3,   6600000, 0.005000),
  ('ID','2024-01-01','ter_a', 4,   7800000, 0.010000),
  ('ID','2024-01-01','ter_a', 5,   8900000, 0.015000),
  ('ID','2024-01-01','ter_a', 6,   9650000, 0.025000),
  ('ID','2024-01-01','ter_a', 7,  10350000, 0.030000),
  ('ID','2024-01-01','ter_a', 8,  12100000, 0.050000),
  ('ID','2024-01-01','ter_a', 9,  15400000, 0.080000),
  ('ID','2024-01-01','ter_a',10,  19500000, 0.120000),
  ('ID','2024-01-01','ter_a',11,  33700000, 0.170000),
  ('ID','2024-01-01','ter_a',12,  45500000, 0.200000),
  ('ID','2024-01-01','ter_b', 1,         0, 0.000000),
  ('ID','2024-01-01','ter_b', 2,   5400000, 0.005000),
  ('ID','2024-01-01','ter_b', 3,   6600000, 0.010000),
  ('ID','2024-01-01','ter_b', 4,   7800000, 0.020000),
  ('ID','2024-01-01','ter_b', 5,   8900000, 0.035000),
  ('ID','2024-01-01','ter_b', 6,   9650000, 0.045000),
  ('ID','2024-01-01','ter_b', 7,  10350000, 0.065000),
  ('ID','2024-01-01','ter_b', 8,  12100000, 0.090000),
  ('ID','2024-01-01','ter_b', 9,  15400000, 0.130000),
  ('ID','2024-01-01','ter_b',10,  19500000, 0.170000),
  ('ID','2024-01-01','ter_b',11,  33700000, 0.230000),
  ('ID','2024-01-01','ter_b',12,  45500000, 0.270000),
  ('ID','2024-01-01','ter_c', 1,         0, 0.000000),
  ('ID','2024-01-01','ter_c', 2,   5400000, 0.010000),
  ('ID','2024-01-01','ter_c', 3,   6600000, 0.020000),
  ('ID','2024-01-01','ter_c', 4,   7800000, 0.035000),
  ('ID','2024-01-01','ter_c', 5,   8900000, 0.050000),
  ('ID','2024-01-01','ter_c', 6,   9650000, 0.060000),
  ('ID','2024-01-01','ter_c', 7,  10350000, 0.100000),
  ('ID','2024-01-01','ter_c', 8,  12100000, 0.130000),
  ('ID','2024-01-01','ter_c', 9,  15400000, 0.170000),
  ('ID','2024-01-01','ter_c',10,  19500000, 0.210000),
  ('ID','2024-01-01','ter_c',11,  33700000, 0.280000),
  ('ID','2024-01-01','ter_c',12,  45500000, 0.320000);

-- BPJS component rates: one complete 12-row matrix. Every effective_from must carry the COMPLETE
-- matrix (the resolver loads the whole set in force at a period — the greatest effective_from <=
-- it — and refuses on any missing component, side, or cap), so a change to one component is a new
-- effective-dated set restating the others unchanged. The JP wage cap is the unreviewed starter
-- value — the statutory reviewer corrects it with a new complete effective set.
INSERT INTO payroll.bpjs_params (country_code, effective_from, component, side, rate, wage_cap) VALUES
  ('ID','2022-01-01','kes',   'employee', 0.010000, 12000000),
  ('ID','2022-01-01','kes',   'employer', 0.040000, 12000000),
  ('ID','2022-01-01','jht',   'employee', 0.020000, NULL),
  ('ID','2022-01-01','jht',   'employer', 0.037000, NULL),
  ('ID','2022-01-01','jp',    'employee', 0.010000, 10547400),
  ('ID','2022-01-01','jp',    'employer', 0.020000, 10547400),
  ('ID','2022-01-01','jkk_1', 'employer', 0.002400, NULL),
  ('ID','2022-01-01','jkk_2', 'employer', 0.005400, NULL),
  ('ID','2022-01-01','jkk_3', 'employer', 0.008900, NULL),
  ('ID','2022-01-01','jkk_4', 'employer', 0.012700, NULL),
  ('ID','2022-01-01','jkk_5', 'employer', 0.017400, NULL),
  ('ID','2022-01-01','jkm',   'employer', 0.003000, NULL);

-- Overtime: workday first hour 1.5x, subsequent hours 2x — per DAY's stretch (the first-hour
-- premium resets daily); rest-day bands seeded but not yet dispatched by the slip builder.
INSERT INTO payroll.overtime_params (country_code, effective_from, day_kind, hour_from, hour_to, multiplier) VALUES
  ('ID','2022-01-01','workday',  1, 1, 1.50),
  ('ID','2022-01-01','workday',  2, NULL, 2.00),
  ('ID','2022-01-01','rest_day', 1, 8, 2.00),
  ('ID','2022-01-01','rest_day', 9, NULL, 3.00);
