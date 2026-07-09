-- Storage-layer backstop for the salary-run money invariants. The schema's @non_negative attribute emits
-- NO CHECK, so every guard lived only in the Rust write path — as strong as "only the engine writes"
-- (maturity council 2026-07-08). These CHECKs defend the invariants against ANY writer, including the
-- generic CRUD/PATCH surface, and pin the proration bound the one-sided clamp missed.
ALTER TABLE payroll.salary_slips
  ADD CONSTRAINT salary_slips_working_days_non_negative CHECK (working_days >= 0),
  ADD CONSTRAINT salary_slips_unpaid_days_non_negative  CHECK (unpaid_days >= 0),
  ADD CONSTRAINT salary_slips_unpaid_within_working     CHECK (unpaid_days <= working_days),
  ADD CONSTRAINT salary_slips_gross_non_negative        CHECK (gross_pay >= 0),
  ADD CONSTRAINT salary_slips_deductions_non_negative   CHECK (total_deductions >= 0),
  ADD CONSTRAINT salary_slips_net_non_negative          CHECK (net_pay >= 0),
  -- The slip identity: net is exactly gross minus deductions (no silent skew).
  ADD CONSTRAINT salary_slips_net_identity              CHECK (net_pay = gross_pay - total_deductions);

ALTER TABLE payroll.salary_slip_lines
  ADD CONSTRAINT salary_slip_lines_amount_non_negative CHECK (amount >= 0);

ALTER TABLE payroll.salary_components
  ADD CONSTRAINT salary_components_amount_non_negative CHECK (amount >= 0);
