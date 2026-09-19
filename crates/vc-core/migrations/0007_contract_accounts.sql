-- A contract holds the money it was given, and in this schema a holder is a
-- `users` row: `assets.user_id` points at one. So the only way the ledger can say
-- where a locked remainder is, is for the contract to have an account of its own.
--
-- Not invented here. The tables this port replaced had `users.contract_id`, and
-- their `deposit_agreements.currency_id` pointed at a `users` row — the sketch of
-- exactly this. They were empty and mis-keyed, so `0004` dropped them along with
-- the column; the column is what was wanted from them.

ALTER TABLE ONLY public.users ADD COLUMN IF NOT EXISTS contract_id bigint;

CREATE UNIQUE INDEX IF NOT EXISTS users_contract_id_index
    ON public.users USING btree (contract_id);

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'users_contract_id_fkey'
      AND conrelid = 'public.users'::regclass) THEN
    ALTER TABLE ONLY public.users
        ADD CONSTRAINT users_contract_id_fkey
            FOREIGN KEY (contract_id) REFERENCES public.contracts(id);
  END IF;
END
$migration$;;

-- Contracts that are already standing get the account they should have had.
INSERT INTO public.users (status, contract_id, inserted_at, updated_at)
SELECT NULL, c.id, now(), now()
  FROM public.contracts c
 WHERE NOT EXISTS (SELECT 1 FROM public.users u WHERE u.contract_id = c.id);

-- And their parties' remainders move into it. Before this migration a remainder
-- lived only in `contract_parties.remaining`, which is where it went when the
-- payer was debited: the currency's supply had lost it. This is the money coming
-- back into the ledger, which is what makes `SUM(assets.amount)` the supply again.
INSERT INTO public.assets (user_id, currency_id, amount, inserted_at, updated_at)
SELECT u.id, c.currency_id, SUM(p.remaining), now(), now()
  FROM public.contracts c
  JOIN public.users u ON u.contract_id = c.id
  JOIN public.contract_parties p ON p.contract_id = c.id AND p.remaining > 0
 WHERE c.status IN ('pending', 'active')
 GROUP BY u.id, c.currency_id
ON CONFLICT (user_id, currency_id)
DO UPDATE SET amount = assets.amount + EXCLUDED.amount,
              updated_at = EXCLUDED.updated_at;
