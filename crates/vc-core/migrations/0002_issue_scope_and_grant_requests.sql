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
    status character varying(255) NOT NULL DEFAULT 'pending',
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    CONSTRAINT grant_requests_status_is_known
        CHECK (status IN ('pending', 'approved', 'denied'))
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

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_requests_pkey'
      AND conrelid = 'public.grant_requests'::regclass) THEN
    ALTER TABLE ONLY public.grant_requests
    ADD CONSTRAINT grant_requests_pkey PRIMARY KEY (id);
  END IF;
END
$migration$;;

-- Asking twice is the same ask: one *pending* request per application and guild.
-- A decided one is remembered rather than reused, so a guild may be asked again
-- after it says no.
CREATE UNIQUE INDEX IF NOT EXISTS grant_requests_pending_index
    ON public.grant_requests USING btree (application_id, guild_id)
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
