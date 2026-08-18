-- Consumer-side dedup inbox for the integration events this module consumes
-- (onboarding.completed, promotion.effective, offboarding.closed). The inbox claim is
-- (consumer, event_id); redeliveries return already-consumed and skip the effect. Without this
-- table the handlers' first claim fails, so every consumed event exhausts its retries and is
-- dropped — the table must exist wherever the module is composed, which is why it lives in a
-- migration rather than runtime bootstrap. Same shape as the framework's backbone-outbox inbox
-- table so hosts and tooling can treat them uniformly.
CREATE TABLE IF NOT EXISTS payroll.inbox_consumed (
  consumer text NOT NULL,
  event_id uuid NOT NULL,
  consumed_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (consumer, event_id) );
