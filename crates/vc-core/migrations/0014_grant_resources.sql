-- Which currencies a grant is for: RFC 8707's `resource`, stored as ids.
--
-- A grant says what an application may do (`grant_scopes`) and nothing about
-- which currency it may do it in. `docs/resources.md` is the design for the
-- second half: the client names the currencies it intends to touch on the ask,
-- and what is stored is the currency's own id — never the URI it arrived as,
-- because the URI is the wire form and an id keeps a grant working when the
-- site moves.
--
-- The URI has two forms, and the collection form means the same thing as naming
-- nothing at all: every currency of the target, now and later. That is why an
-- empty set is the "all of them" case below rather than an error, and why a
-- grant written before this migration existed keeps meaning what it meant.
--
-- Additive and non-breaking the way 0002 and 0004 are: the Elixir neither knows
-- nor reads any of this.

-- The ask carries what was asked for in one row, the shape `scopes` already has.
ALTER TABLE public.grant_requests
    ADD COLUMN IF NOT EXISTS resources bigint[] NOT NULL DEFAULT '{}';

-- The authorization code carries it too, so the browser flow's resource
-- survives from the consent screen to the exchange that writes the grant —
-- without this column the ids would have nowhere to wait between the two.
ALTER TABLE public.authorization_codes
    ADD COLUMN IF NOT EXISTS resources bigint[] NOT NULL DEFAULT '{}';

-- `grant_resources`: what a grant carries once somebody has approved it, in
-- rows a read can join — the same shape `grant_scopes` has, and for the same
-- reason.
CREATE TABLE IF NOT EXISTS public.grant_resources (
    id bigint NOT NULL,
    grant_id bigint,
    currency_id bigint,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);

CREATE SEQUENCE IF NOT EXISTS public.grant_resources_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.grant_resources_id_seq OWNED BY public.grant_resources.id;

ALTER TABLE ONLY public.grant_resources
    ALTER COLUMN id SET DEFAULT nextval('public.grant_resources_id_seq'::regclass);

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_resources_pkey'
      AND conrelid = 'public.grant_resources'::regclass) THEN
    ALTER TABLE ONLY public.grant_resources
    ADD CONSTRAINT grant_resources_pkey PRIMARY KEY (id);
  END IF;
END
$migration$;;

-- A grant's own deletion takes its resource rows with it, exactly as it takes
-- its scopes.
DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_resources_grant_id_fkey'
      AND conrelid = 'public.grant_resources'::regclass) THEN
    ALTER TABLE ONLY public.grant_resources
    ADD CONSTRAINT grant_resources_grant_id_fkey FOREIGN KEY (grant_id)
        REFERENCES public.grants(id) ON UPDATE CASCADE ON DELETE CASCADE;
  END IF;
END
$migration$;;

-- A currency's own deletion takes its resource rows with it: a permission in a
-- currency that no longer exists is not a permission.
DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_resources_currency_id_fkey'
      AND conrelid = 'public.grant_resources'::regclass) THEN
    ALTER TABLE ONLY public.grant_resources
    ADD CONSTRAINT grant_resources_currency_id_fkey FOREIGN KEY (currency_id)
        REFERENCES public.currencies(id) ON UPDATE CASCADE ON DELETE CASCADE;
  END IF;
END
$migration$;;

CREATE UNIQUE INDEX IF NOT EXISTS grant_resources_grant_id_currency_id_index
    ON public.grant_resources USING btree (grant_id, currency_id);

CREATE UNIQUE INDEX IF NOT EXISTS grant_resources_id_index
    ON public.grant_resources USING btree (id);

CREATE INDEX IF NOT EXISTS grant_resources_grant_id_index
    ON public.grant_resources USING btree (grant_id);
