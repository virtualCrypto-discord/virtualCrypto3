-- Snapshots are written by the same transaction as the balance movement.
-- Old rows remain NULL: current balances cannot reconstruct historic balances.
ALTER TABLE currency_payment_histories
    ADD COLUMN sender_balance_after bigint CHECK (sender_balance_after >= 0),
    ADD COLUMN receiver_balance_after bigint CHECK (receiver_balance_after >= 0);
ALTER TABLE currency_given_histories
    ADD COLUMN receiver_balance_after bigint CHECK (receiver_balance_after >= 0),
    ADD COLUMN pool_balance_after bigint CHECK (pool_balance_after >= 0);
