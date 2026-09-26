-- Keep the account/key uniqueness rule: changing namespaces could execute a
-- previously completed payment again. New rows record who may replay them;
-- NULL marks legacy rows whose operation and delegation were not recorded.
ALTER TABLE payments_idempotency ADD COLUMN replay_context text;
