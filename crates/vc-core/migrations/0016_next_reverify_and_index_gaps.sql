-- The clock's queries, and one list, given something an index can serve.
--
-- A query audit found three statements that scan a table because their filter or
-- their order is not a column. Two of them are the clock's — the webhook
-- re-verification sweep every minute and the idempotency purge — and the third is
-- the guild's authorized-application list, which the same audit caught. Each is
-- fixed by giving the query what it actually asks for rather than by asking the
-- planner to be cleverer, and they are one migration because they are one class of
-- defect found together.
--
-- **The sweep is the real one.** `applications_webhook_verified_at_index` (`0010`)
-- is on `webhook_verified_at`, but the sweep filters and orders by
-- `GREATEST(webhook_verified_at, webhook_failed_at)`, wants never-checked rows
-- first (`NULLS FIRST`, where the index is `NULLS LAST`), and its `OR` rules out a
-- single range scan — so the index cannot serve it and every tick sequential-scans
-- and sorts `applications`. What is stored instead is the time the row is next
-- due, which is the sweep's whole predicate as a column: one comparison, one
-- direction, and one partial index over the rows that named a webhook. `0010`'s
-- index is then dropped — nothing reads `webhook_verified_at` as an ordering key
-- any more, and the column stays as the history the API answers with.
--
-- As with `0003` and `0010`, `next_reverify_at` is this service's own column on a
-- shared table: the Elixir neither writes nor reads it.
ALTER TABLE public.applications
    ADD COLUMN IF NOT EXISTS next_reverify_at timestamp(0) without time zone;

-- Backfill, in the sweep's own terms. An application that names a webhook and has
-- never been checked is due now — which is what `NULLS FIRST` selected in `0010`,
-- and what registering a webhook sets. One that has been checked is due a week
-- after that check, the same seven days the sweep used to require via
-- `stale_before`, so the cut-over does not re-check every webhook in one pass.
-- `webhook_url IS NULL` leaves the column NULL, which is also what removing a
-- webhook sets it to.
--
-- The clock is UTC (`now() AT TIME ZONE 'UTC'`, not a bare `now()`): the column is
-- a `timestamp without time zone` that the service reads as UTC, so a bare `now()`
-- would schedule a never-checked webhook by the server's own offset — ahead of the
-- sweep's clock and not due at all for that much longer where the server is not in
-- UTC. It is truncated to the second, which is what `model::utc_now` writes, so a
-- never-checked row is due on the very next tick rather than one after it.
UPDATE public.applications
   SET next_reverify_at =
           COALESCE(GREATEST(webhook_verified_at, webhook_failed_at) + interval '7 days',
                    date_trunc('second', now() AT TIME ZONE 'UTC'))
 WHERE webhook_url IS NOT NULL
   AND next_reverify_at IS NULL;

CREATE INDEX IF NOT EXISTS applications_next_reverify_at_index
    ON public.applications USING btree (next_reverify_at)
    WHERE webhook_url IS NOT NULL;

-- Superseded by the index above; kept for nothing would be a write cost on every
-- verification with no reader.
DROP INDEX IF EXISTS public.applications_webhook_verified_at_index;

-- The guild's authorized-application list filters `grants` by `guild_id` with a
-- count and a page, and both pair indexes on the table —
-- `grants_application_id_guild_id_index` (`0001`) and
-- `grants_application_id_target_index` (`0013`) — lead on `application_id`, so
-- neither can serve it and the count and the page scan `grants`.
CREATE INDEX IF NOT EXISTS grants_guild_id_index
    ON public.grants USING btree (guild_id);

-- The per-minute purge deletes the idempotency keys whose week is up, and
-- `payments_idempotency.expires` — on a table that grows with every payment — had
-- no index at all, so `DELETE FROM payments_idempotency WHERE expires < $1` was a
-- sequential scan every minute.
CREATE INDEX IF NOT EXISTS payments_idempotency_expires_index
    ON public.payments_idempotency USING btree (expires);
