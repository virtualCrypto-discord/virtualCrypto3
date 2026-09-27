-- A webhook check runs without holding an application row lock during HTTP.
-- Advance this revision when the URL changes or a check is recorded, so a
-- delayed result cannot overwrite a replacement's schedule or another result.
-- Unlike timestamp(0), this also distinguishes changes within the same second,
-- including removing a webhook and restoring the identical URL.
ALTER TABLE public.applications
    ADD COLUMN IF NOT EXISTS webhook_revision bigint NOT NULL DEFAULT 0;
