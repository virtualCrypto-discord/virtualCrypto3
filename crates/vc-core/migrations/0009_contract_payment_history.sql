-- What a contract payment was, which the ledger did not say.
--
-- `currency_payment_histories` is the Elixir's table and holds one row per
-- movement: who sent, who received, how much, in which currency, when. A
-- contract payment writes one row per party whose remainder was drawn on, with
-- that party's own account as the sender — the money's path into the receiver's
-- balance rather than a note that an application paid (`vc_core::contract::pay`).
-- Every column of such a row means exactly what it means for an ordinary
-- transfer, so **nothing in the table says which contract it came from**, and a
-- statement per contract cannot be read out of it.
--
-- The column is what it takes, and it is nullable because the table is shared
-- with the Elixir, which neither writes nor reads it: the rows it inserts keep
-- NULL, and that is also the honest answer for an ordinary transfer here.
--
-- **The rows already written are not backfilled.** They cannot be: a contract
-- payment of the past is byte for byte an ordinary transfer, and a guess built
-- out of "the sender is one of the contract's parties, the receiver is the
-- contract's receiver, the currency matches" would file somebody's own payment
-- under a contract it never belonged to. A contract's statement therefore begins
-- at this migration and says so.

ALTER TABLE public.currency_payment_histories
    ADD COLUMN IF NOT EXISTS contract_id bigint;

-- A statement reads one contract's rows newest first, so the pair is what the
-- index is for: the contract to narrow by, the id to order and page with.
CREATE INDEX IF NOT EXISTS currency_payment_histories_contract_id_id_index
    ON public.currency_payment_histories USING btree (contract_id, id);

-- No cascade, and the same reason `contracts_currency_id_fkey` has none: a
-- contract with payments hanging off it is a contract whose deletion has to say
-- what happens to what it moved, rather than one that silently takes the ledger
-- with it.
DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'currency_payment_histories_contract_id_fkey'
      AND conrelid = 'public.currency_payment_histories'::regclass) THEN
    ALTER TABLE ONLY public.currency_payment_histories
    ADD CONSTRAINT currency_payment_histories_contract_id_fkey FOREIGN KEY (contract_id)
        REFERENCES public.contracts(id);
  END IF;
END
$migration$;;
