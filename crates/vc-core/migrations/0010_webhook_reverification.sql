-- Whether an application's webhook still answers, which this service could not
-- remember.
--
-- The handshake — a PING signed with the application's own key that must come
-- back `200`, and one signed with a key nobody has that must come back `401` —
-- runs when a webhook is registered and when it is edited, and nothing has run
-- it since the port (`docs/known-gaps.md`). Re-running it needs somewhere for
-- the answer to go: an application that has stopped answering is a fact about a
-- row, and a line in a log is not a row.
--
-- Two columns, because two different questions are asked of them. When it last
-- **passed** is what tells the job who to look at next — NULL means never
-- checked, which is every application whose webhook was verified at
-- registration before this migration ran. When it last **failed** is what
-- someone reads afterwards.
--
-- A failure does not clear the pass, and neither of them takes the webhook away.
-- A webhook that missed one handshake is not a webhook this service stops
-- delivering to: what a delivery carries is a decision about someone's money,
-- and a transient outage is not a reason to stop telling an application about
-- it. What the columns are for is the record, and the reason to re-check sooner.
--
-- They are this service's own columns on a shared table, like the ones `0003`
-- added: the Elixir neither writes nor reads them.
ALTER TABLE public.applications
    ADD COLUMN IF NOT EXISTS webhook_verified_at timestamp(0) without time zone,
    ADD COLUMN IF NOT EXISTS webhook_failed_at timestamp(0) without time zone;

-- Who the job looks at next: the applications that named a webhook, oldest
-- check first, never-checked ahead of the rest.
CREATE INDEX IF NOT EXISTS applications_webhook_verified_at_index
    ON public.applications USING btree (webhook_verified_at)
    WHERE webhook_url IS NOT NULL;
