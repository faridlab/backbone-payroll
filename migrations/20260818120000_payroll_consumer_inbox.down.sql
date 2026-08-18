-- Inverse of 20260818120000_payroll_consumer_inbox.up.sql.
-- Drops the consumer dedup inbox. Any composed deployment relying on the inbox for redelivery
-- dedup loses that protection once the table is gone — only run this when decommissioning the
-- module's event consumers.
DROP TABLE IF EXISTS payroll.inbox_consumed;
