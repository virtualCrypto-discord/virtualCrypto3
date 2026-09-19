-- A contract that ran out of time is its own ending, and the service now acts on
-- it: what the parties had left is refunded and the contract is marked `expired`
-- rather than `canceled` — the deadline did it, not anyone in it.
--
-- Not from the Elixir, which has no contract flow at all. This is the status the
-- design in `docs/contracts.md` grew when the scheduler arrived.

DO $migration$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'contracts_status_is_known'
      AND conrelid = 'public.contracts'::regclass) THEN
    ALTER TABLE ONLY public.contracts DROP CONSTRAINT contracts_status_is_known;
  END IF;
END
$migration$;;

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_status_is_known
        CHECK (status IN ('pending', 'active', 'canceled', 'expired'));

-- What the settling job asks for: the contracts still standing whose deadline has
-- passed. A partial index rather than one over the column, because the row that
-- is already over is one the job never looks at again.
CREATE INDEX IF NOT EXISTS contracts_unsettled_expiry_index
    ON public.contracts USING btree (expires_at)
    WHERE status IN ('pending', 'active') AND expires_at IS NOT NULL;
