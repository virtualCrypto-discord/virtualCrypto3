-- Contract decisions are the fourth deliverable event type, so the default
-- subscription — what a registration that names none gets — grows to hold it.
--
-- **Only the default.** Existing rows keep the set they wrote: an application
-- that subscribed to `{2, 3}` asked for two kinds of event, and a fourth kind it
-- never named is not something to send it. It can ask for `4` with an edit, and
-- the deliveries it already receives are unchanged either way.
ALTER TABLE public.applications
    ALTER COLUMN subscribed_events SET DEFAULT '{2,3,4}';
