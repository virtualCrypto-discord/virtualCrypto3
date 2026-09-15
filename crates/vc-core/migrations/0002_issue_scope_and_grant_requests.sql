-- The `vc.issue` scope, and the requests a guild decides on before granting it.
--
-- Not from the Elixir: it issues no guild tokens and has no endpoint that issues
-- from a pool, so neither the scope nor this table has a counterpart in the Ecto
-- schema that 0001 was generated from. Both are additive, which is what lets the
-- two services share a database while the rewrite is finished.

-- `ALTER TYPE ... ADD VALUE` is the one statement here that a server must be
-- PostgreSQL 12 for, and it is not used in the same transaction: the table below
-- names no scope.
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.issue';

CREATE TABLE IF NOT EXISTS public.grant_requests (
    id bigint NOT NULL,
    application_id bigint NOT NULL,
    guild_id bigint NOT NULL,
    scopes text[] NOT NULL,
    device_code uuid NOT NULL,
    user_code character varying(8) NOT NULL,
    expires_in bigint NOT NULL,
    status character varying(255) NOT NULL DEFAULT 'pending',
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    CONSTRAINT grant_requests_status_is_known
        CHECK (status IN ('pending', 'approved'))
);

CREATE SEQUENCE IF NOT EXISTS public.grant_requests_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.grant_requests_id_seq OWNED BY public.grant_requests.id;

ALTER TABLE ONLY public.grant_requests
    ALTER COLUMN id SET DEFAULT nextval('public.grant_requests_id_seq'::regclass);

ALTER TABLE public.grant_requests
    ADD COLUMN IF NOT EXISTS scopes text[] NOT NULL DEFAULT '{}',
    ADD COLUMN IF NOT EXISTS device_code uuid,
    ADD COLUMN IF NOT EXISTS user_code character varying(8),
    ADD COLUMN IF NOT EXISTS expires_in bigint NOT NULL DEFAULT 600;

-- Existing rows predate the device flow and have no codes: backfill them so
-- the new `NOT NULL`s below hold.
UPDATE public.grant_requests
   SET device_code = gen_random_uuid()
 WHERE device_code IS NULL;

UPDATE public.grant_requests
   SET user_code = substring(md5(random()::text) from 1 for 8)
 WHERE user_code IS NULL;

ALTER TABLE public.grant_requests
    ALTER COLUMN device_code SET NOT NULL,
    ALTER COLUMN user_code SET NOT NULL;

DO $migration$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_requests_status_is_known') THEN
    ALTER TABLE ONLY public.grant_requests DROP CONSTRAINT grant_requests_status_is_known;
  END IF;
END
$migration$;;

ALTER TABLE ONLY public.grant_requests
    ADD CONSTRAINT grant_requests_status_is_known
        CHECK (status IN ('pending', 'approved')) NOT VALID;

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_requests_pkey'
      AND conrelid = 'public.grant_requests'::regclass) THEN
    ALTER TABLE ONLY public.grant_requests
    ADD CONSTRAINT grant_requests_pkey PRIMARY KEY (id);
  END IF;
END
$migration$;;

-- A decided one is superseded by the decision itself: an approved request lives
-- on as the grant it wrote, and approval is what the application polls for, so
-- asking again is a new row either way. Denial is not recorded at all —
-- an unapproved request simply stays pending until it expires.
CREATE UNIQUE INDEX IF NOT EXISTS grant_requests_pending_index
    ON public.grant_requests USING btree (application_id, guild_id)
    WHERE status = 'pending';

-- A `user_code` names one pending ask in one guild: it is what the
-- administrator types into `/grant approve`, so it must pick out exactly one.
CREATE UNIQUE INDEX IF NOT EXISTS grant_requests_user_code_index
    ON public.grant_requests USING btree (guild_id, user_code)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS grant_requests_guild_id_index
    ON public.grant_requests USING btree (guild_id);

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_requests_application_id_fkey'
      AND conrelid = 'public.grant_requests'::regclass) THEN
    ALTER TABLE ONLY public.grant_requests
    ADD CONSTRAINT grant_requests_application_id_fkey FOREIGN KEY (application_id)
        REFERENCES public.applications(id) ON UPDATE CASCADE ON DELETE CASCADE;
  END IF;
END
$migration$;;
