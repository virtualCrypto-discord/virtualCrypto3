-- Which event types an application wants delivered to its webhook.
--
-- Not from the Elixir: it delivers every claim update to every application with
-- a webhook, and an application that only cares about one kind of event cannot
-- say so. The column holds the `type` values from the delivery bodies —
-- `2` for a claim update, `3` for a grant decision — and it is exactly what it
-- says: checked is sent, unchecked is not, and empty is nothing.
--
-- The default is everything, spelled out, so existing rows keep receiving what
-- they received: a migration that unsubscribed anyone would be a breaking
-- change wearing a schema change's clothes.

ALTER TABLE public.applications
    ADD COLUMN IF NOT EXISTS subscribed_events bigint[] NOT NULL DEFAULT '{2,3}';
