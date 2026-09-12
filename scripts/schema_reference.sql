--
-- PostgreSQL database dump
--

\restrict h3eRyUFGfygQGVa6Li0DdQFLxhgN5fbzSjyigIPOZyvRYuYt4X1eyYH7OBaFzfa

-- Dumped from database version 17.4
-- Dumped by pg_dump version 17.11

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: openid_connect_application_type; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.openid_connect_application_type AS ENUM (
    'native',
    'web'
);


--
-- Name: openid_connect_grant_types; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.openid_connect_grant_types AS ENUM (
    'unbound_authorization_code',
    'authorization_code',
    'refresh_token'
);


--
-- Name: openid_connect_response_types; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.openid_connect_response_types AS ENUM (
    'code'
);


--
-- Name: virtual_crypto_claim_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.virtual_crypto_claim_status AS ENUM (
    'pending',
    'approved',
    'denied',
    'canceled'
);


--
-- Name: virtual_crypto_scope_type; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.virtual_crypto_scope_type AS ENUM (
    'openid'
);


--
-- Name: claim_metadata_remove_row(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.claim_metadata_remove_row() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  DELETE FROM claim_metadata WHERE id=NEW.id;
  RETURN NULL;
END;
$$;


--
-- Name: claim_metadata_strip_nulls(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.claim_metadata_strip_nulls() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
  BEGIN
    UPDATE claim_metadata SET metadata=jsonb_strip_nulls(NEW.metadata) WHERE id=NEW.id;
    RETURN NULL;
  END;
$$;


--
-- Name: delete_asset_by_id(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.delete_asset_by_id() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
  BEGIN
    DELETE FROM assets WHERE assets.id = NEW.id;
    RETURN NULL;
  END;
$$;


--
-- Name: metadata_limitation(jsonb); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.metadata_limitation(json_obj jsonb) RETURNS boolean
    LANGUAGE plpgsql
    AS $$
DECLARE
entry RECORD;
key_count INTEGER;
BEGIN
  key_count := 0;
  FOR entry IN (
    SELECT * FROM jsonb_each(json_obj)
  )
  LOOP
    key_count := key_count + 1;
    IF length(entry.key::text) > 40 OR jsonb_typeof(entry.value) != 'string' OR length(entry.value#>>'{}') > 500 OR key_count > 50 THEN
      RETURN FALSE;
    END IF;
  END LOOP;
  RETURN TRUE;
END;
$$;


--
-- Name: metadata_limitation_trigger(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.metadata_limitation_trigger() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
  BEGIN
    RAISE check_violation;
  END;
$$;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: access_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.access_tokens (
    id bigint NOT NULL,
    grant_id bigint,
    expires timestamp(0) without time zone,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    token_id uuid
);


--
-- Name: access_tokens_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.access_tokens_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: access_tokens_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.access_tokens_id_seq OWNED BY public.access_tokens.id;


--
-- Name: applications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.applications (
    id bigint NOT NULL,
    status integer,
    client_id uuid,
    client_secret character varying(255),
    response_types public.openid_connect_response_types[] DEFAULT ARRAY['code'::public.openid_connect_response_types],
    grant_types public.openid_connect_grant_types[] DEFAULT ARRAY['authorization_code'::public.openid_connect_grant_types],
    application_type public.openid_connect_application_type DEFAULT 'web'::public.openid_connect_application_type,
    client_name character varying(255),
    client_uri character varying(255),
    logo_uri character varying(255),
    owner_discord_id bigint,
    discord_support_server_invite_slug character varying(255),
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    webhook_url character varying(2048),
    public_key bytea NOT NULL,
    private_key bytea NOT NULL
);


--
-- Name: applications_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.applications_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: applications_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.applications_id_seq OWNED BY public.applications.id;


--
-- Name: assets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assets (
    id bigint NOT NULL,
    amount bigint,
    user_id bigint,
    currency_id bigint,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    CONSTRAINT amount_must_be_positive CHECK ((amount > 0))
);


--
-- Name: assets_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.assets_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: assets_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.assets_id_seq OWNED BY public.assets.id;


--
-- Name: authorization_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.authorization_codes (
    id bigint NOT NULL,
    code character varying(255),
    redirect_uri character varying(255),
    application_id bigint,
    guild_id bigint,
    scopes public.virtual_crypto_scope_type[],
    expires timestamp(0) without time zone,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: authorization_codes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.authorization_codes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: authorization_codes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.authorization_codes_id_seq OWNED BY public.authorization_codes.id;


--
-- Name: claim_metadata; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.claim_metadata (
    id bigint NOT NULL,
    claim_id bigint NOT NULL,
    claimant_user_id bigint NOT NULL,
    payer_user_id bigint NOT NULL,
    owner_user_id bigint NOT NULL,
    metadata jsonb NOT NULL,
    CONSTRAINT metadata_must_be_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT metadata_owner_is_must_related_user CHECK (((owner_user_id = claimant_user_id) OR (owner_user_id = payer_user_id)))
);


--
-- Name: claim_metadata_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.claim_metadata_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: claim_metadata_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.claim_metadata_id_seq OWNED BY public.claim_metadata.id;


--
-- Name: claims; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.claims (
    id bigint NOT NULL,
    amount bigint,
    status public.virtual_crypto_claim_status,
    claimant_user_id bigint NOT NULL,
    payer_user_id bigint NOT NULL,
    currency_id bigint NOT NULL,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: claims_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.claims_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: claims_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.claims_id_seq OWNED BY public.claims.id;


--
-- Name: currencies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.currencies (
    id bigint NOT NULL,
    name character varying(255),
    unit character varying(255),
    guild_id bigint,
    pool_amount bigint,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    CONSTRAINT pool_amount_must_not_be_negative CHECK ((pool_amount >= 0))
);


--
-- Name: currency_given_histories; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.currency_given_histories (
    id bigint NOT NULL,
    amount bigint,
    receiver_id bigint,
    currency_id bigint,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    "time" timestamp(0) without time zone
);


--
-- Name: currency_payment_histories; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.currency_payment_histories (
    id bigint NOT NULL,
    amount bigint,
    sender_id bigint,
    receiver_id bigint,
    currency_id bigint,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    "time" timestamp(0) without time zone
);


--
-- Name: discord_users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.discord_users (
    id bigint NOT NULL,
    discord_user_id bigint,
    refresh_token character varying(255),
    token character varying(255),
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    expires timestamp(0) without time zone
);


--
-- Name: discord_users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.discord_users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: discord_users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.discord_users_id_seq OWNED BY public.discord_users.id;


--
-- Name: grant_scopes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.grant_scopes (
    id bigint NOT NULL,
    grant_id bigint,
    scope public.virtual_crypto_scope_type,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: grant_scopes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.grant_scopes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: grant_scopes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.grant_scopes_id_seq OWNED BY public.grant_scopes.id;


--
-- Name: grants; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.grants (
    id bigint NOT NULL,
    application_id bigint,
    guild_id bigint,
    latest_code character varying(255),
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: grants_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.grants_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: grants_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.grants_id_seq OWNED BY public.grants.id;


--
-- Name: info_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.info_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: info_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.info_id_seq OWNED BY public.currencies.id;


--
-- Name: money_given_historys_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.money_given_historys_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: money_given_historys_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.money_given_historys_id_seq OWNED BY public.currency_given_histories.id;


--
-- Name: money_payment_historys_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.money_payment_historys_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: money_payment_historys_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.money_payment_historys_id_seq OWNED BY public.currency_payment_histories.id;


--
-- Name: payments_idempotency; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.payments_idempotency (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    idempotency_key bytea NOT NULL,
    expires timestamp(0) without time zone NOT NULL,
    http_status integer,
    body jsonb,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: payments_idempotency_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.payments_idempotency_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: payments_idempotency_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.payments_idempotency_id_seq OWNED BY public.payments_idempotency.id;


--
-- Name: redirect_uris; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.redirect_uris (
    id bigint NOT NULL,
    application_id bigint,
    redirect_uri character varying(255),
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: redirect_uris_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.redirect_uris_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: redirect_uris_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.redirect_uris_id_seq OWNED BY public.redirect_uris.id;


--
-- Name: refresh_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.refresh_tokens (
    id bigint NOT NULL,
    grant_id bigint,
    expires timestamp(0) without time zone,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    token_id uuid
);


--
-- Name: refresh_tokens_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.refresh_tokens_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: refresh_tokens_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.refresh_tokens_id_seq OWNED BY public.refresh_tokens.id;


--
-- Name: schema_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.schema_migrations (
    version bigint NOT NULL,
    inserted_at timestamp(0) without time zone
);


--
-- Name: user_access_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_access_tokens (
    id bigint NOT NULL,
    user_id bigint,
    token_id uuid,
    expires timestamp(0) without time zone,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL
);


--
-- Name: user_access_tokens_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_access_tokens_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_access_tokens_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_access_tokens_id_seq OWNED BY public.user_access_tokens.id;


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id integer NOT NULL,
    status integer,
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    discord_id bigint,
    application_id bigint
);


--
-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;


--
-- Name: access_tokens id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.access_tokens ALTER COLUMN id SET DEFAULT nextval('public.access_tokens_id_seq'::regclass);


--
-- Name: applications id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.applications ALTER COLUMN id SET DEFAULT nextval('public.applications_id_seq'::regclass);


--
-- Name: assets id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assets ALTER COLUMN id SET DEFAULT nextval('public.assets_id_seq'::regclass);


--
-- Name: authorization_codes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.authorization_codes ALTER COLUMN id SET DEFAULT nextval('public.authorization_codes_id_seq'::regclass);


--
-- Name: claim_metadata id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata ALTER COLUMN id SET DEFAULT nextval('public.claim_metadata_id_seq'::regclass);


--
-- Name: claims id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claims ALTER COLUMN id SET DEFAULT nextval('public.claims_id_seq'::regclass);


--
-- Name: currencies id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currencies ALTER COLUMN id SET DEFAULT nextval('public.info_id_seq'::regclass);


--
-- Name: currency_given_histories id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_given_histories ALTER COLUMN id SET DEFAULT nextval('public.money_given_historys_id_seq'::regclass);


--
-- Name: currency_payment_histories id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_payment_histories ALTER COLUMN id SET DEFAULT nextval('public.money_payment_historys_id_seq'::regclass);


--
-- Name: discord_users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_users ALTER COLUMN id SET DEFAULT nextval('public.discord_users_id_seq'::regclass);


--
-- Name: grant_scopes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.grant_scopes ALTER COLUMN id SET DEFAULT nextval('public.grant_scopes_id_seq'::regclass);


--
-- Name: grants id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.grants ALTER COLUMN id SET DEFAULT nextval('public.grants_id_seq'::regclass);


--
-- Name: payments_idempotency id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payments_idempotency ALTER COLUMN id SET DEFAULT nextval('public.payments_idempotency_id_seq'::regclass);


--
-- Name: redirect_uris id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redirect_uris ALTER COLUMN id SET DEFAULT nextval('public.redirect_uris_id_seq'::regclass);


--
-- Name: refresh_tokens id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens ALTER COLUMN id SET DEFAULT nextval('public.refresh_tokens_id_seq'::regclass);


--
-- Name: user_access_tokens id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_access_tokens ALTER COLUMN id SET DEFAULT nextval('public.user_access_tokens_id_seq'::regclass);


--
-- Name: users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);


--
-- Name: access_tokens access_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.access_tokens
    ADD CONSTRAINT access_tokens_pkey PRIMARY KEY (id);


--
-- Name: applications applications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.applications
    ADD CONSTRAINT applications_pkey PRIMARY KEY (id);


--
-- Name: assets assets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assets
    ADD CONSTRAINT assets_pkey PRIMARY KEY (id);


--
-- Name: authorization_codes authorization_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.authorization_codes
    ADD CONSTRAINT authorization_codes_pkey PRIMARY KEY (id);


--
-- Name: claim_metadata claim_metadata_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata
    ADD CONSTRAINT claim_metadata_pkey PRIMARY KEY (id);


--
-- Name: claims claims_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claims
    ADD CONSTRAINT claims_pkey PRIMARY KEY (id);


--
-- Name: claims claims_unique_ordered_claim_metadata_fk; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claims
    ADD CONSTRAINT claims_unique_ordered_claim_metadata_fk UNIQUE (id, claimant_user_id, payer_user_id);


--
-- Name: discord_users discord_users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_users
    ADD CONSTRAINT discord_users_pkey PRIMARY KEY (id);


--
-- Name: grant_scopes grant_scopes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.grant_scopes
    ADD CONSTRAINT grant_scopes_pkey PRIMARY KEY (id);


--
-- Name: grants grants_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.grants
    ADD CONSTRAINT grants_pkey PRIMARY KEY (id);


--
-- Name: currencies info_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currencies
    ADD CONSTRAINT info_pkey PRIMARY KEY (id);


--
-- Name: currency_given_histories money_given_historys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_given_histories
    ADD CONSTRAINT money_given_historys_pkey PRIMARY KEY (id);


--
-- Name: currency_payment_histories money_payment_historys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_payment_histories
    ADD CONSTRAINT money_payment_historys_pkey PRIMARY KEY (id);


--
-- Name: payments_idempotency payments_idempotency_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payments_idempotency
    ADD CONSTRAINT payments_idempotency_pkey PRIMARY KEY (id);


--
-- Name: redirect_uris redirect_uris_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redirect_uris
    ADD CONSTRAINT redirect_uris_pkey PRIMARY KEY (id);


--
-- Name: refresh_tokens refresh_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_pkey PRIMARY KEY (id);


--
-- Name: schema_migrations schema_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (version);


--
-- Name: user_access_tokens user_access_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_access_tokens
    ADD CONSTRAINT user_access_tokens_pkey PRIMARY KEY (id);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: access_tokens_expires_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX access_tokens_expires_index ON public.access_tokens USING btree (expires);


--
-- Name: access_tokens_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX access_tokens_id_index ON public.access_tokens USING btree (id);


--
-- Name: access_tokens_token_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX access_tokens_token_id_index ON public.access_tokens USING btree (token_id);


--
-- Name: applications_client_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX applications_client_id_index ON public.applications USING btree (client_id);


--
-- Name: applications_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX applications_id_index ON public.applications USING btree (id);


--
-- Name: applications_owner_discord_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX applications_owner_discord_id_index ON public.applications USING btree (owner_discord_id);


--
-- Name: assets_money_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assets_money_id_index ON public.assets USING btree (currency_id);


--
-- Name: assets_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assets_user_id_index ON public.assets USING btree (user_id);


--
-- Name: assets_user_id_money_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX assets_user_id_money_id_index ON public.assets USING btree (user_id, currency_id);


--
-- Name: authorization_codes_code_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX authorization_codes_code_index ON public.authorization_codes USING btree (code);


--
-- Name: authorization_codes_expires_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX authorization_codes_expires_index ON public.authorization_codes USING btree (expires);


--
-- Name: authorization_codes_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX authorization_codes_id_index ON public.authorization_codes USING btree (id);


--
-- Name: claim_metadata_claim_id_owner_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX claim_metadata_claim_id_owner_user_id_index ON public.claim_metadata USING btree (claim_id, owner_user_id);


--
-- Name: claims_claimant_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX claims_claimant_user_id_index ON public.claims USING btree (claimant_user_id);


--
-- Name: claims_payer_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX claims_payer_user_id_index ON public.claims USING btree (payer_user_id);


--
-- Name: discord_users_discord_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX discord_users_discord_user_id_index ON public.discord_users USING btree (discord_user_id);


--
-- Name: discord_users_refresh_token_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX discord_users_refresh_token_index ON public.discord_users USING btree (refresh_token);


--
-- Name: discord_users_token_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX discord_users_token_index ON public.discord_users USING btree (token);


--
-- Name: grant_scopes_grant_id_scope_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX grant_scopes_grant_id_scope_index ON public.grant_scopes USING btree (grant_id, scope);


--
-- Name: grant_scopes_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX grant_scopes_id_index ON public.grant_scopes USING btree (id);


--
-- Name: grants_application_id_guild_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX grants_application_id_guild_id_index ON public.grants USING btree (application_id, guild_id);


--
-- Name: grants_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX grants_id_index ON public.grants USING btree (id);


--
-- Name: grants_latest_code_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX grants_latest_code_index ON public.grants USING btree (latest_code);


--
-- Name: info_guild_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX info_guild_id_index ON public.currencies USING btree (guild_id);


--
-- Name: info_name_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX info_name_index ON public.currencies USING btree (name);


--
-- Name: info_unit_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX info_unit_index ON public.currencies USING btree (unit);


--
-- Name: money_given_historys_money_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX money_given_historys_money_id_index ON public.currency_given_histories USING btree (currency_id);


--
-- Name: money_given_historys_receiver_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX money_given_historys_receiver_id_index ON public.currency_given_histories USING btree (receiver_id);


--
-- Name: money_payment_historys_money_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX money_payment_historys_money_id_index ON public.currency_payment_histories USING btree (currency_id);


--
-- Name: money_payment_historys_receiver_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX money_payment_historys_receiver_id_index ON public.currency_payment_histories USING btree (receiver_id);


--
-- Name: money_payment_historys_sender_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX money_payment_historys_sender_id_index ON public.currency_payment_histories USING btree (sender_id);


--
-- Name: payments_idempotency_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX payments_idempotency_id_index ON public.payments_idempotency USING btree (id);


--
-- Name: payments_idempotency_idempotency_key_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX payments_idempotency_idempotency_key_user_id_index ON public.payments_idempotency USING btree (idempotency_key, user_id);


--
-- Name: redirect_uris_application_id_redirect_uri_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX redirect_uris_application_id_redirect_uri_index ON public.redirect_uris USING btree (application_id, redirect_uri);


--
-- Name: refresh_tokens_expires_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX refresh_tokens_expires_index ON public.refresh_tokens USING btree (expires);


--
-- Name: refresh_tokens_grant_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX refresh_tokens_grant_id_index ON public.refresh_tokens USING btree (grant_id);


--
-- Name: refresh_tokens_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX refresh_tokens_id_index ON public.refresh_tokens USING btree (id);


--
-- Name: refresh_tokens_token_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX refresh_tokens_token_id_index ON public.refresh_tokens USING btree (token_id);


--
-- Name: text_claims_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX text_claims_id ON public.claims USING btree (((id)::text));


--
-- Name: user_access_tokens_expires_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_access_tokens_expires_index ON public.user_access_tokens USING btree (expires);


--
-- Name: user_access_tokens_token_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX user_access_tokens_token_id_index ON public.user_access_tokens USING btree (token_id);


--
-- Name: user_access_tokens_user_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_access_tokens_user_id_index ON public.user_access_tokens USING btree (user_id);


--
-- Name: users_application_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX users_application_id_index ON public.users USING btree (application_id);


--
-- Name: users_discord_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX users_discord_id_index ON public.users USING btree (discord_id);


--
-- Name: users_id_index; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX users_id_index ON public.users USING btree (id);


--
-- Name: claim_metadata _remove_entry_if_empty; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER _remove_entry_if_empty BEFORE UPDATE OF metadata ON public.claim_metadata FOR EACH ROW WHEN (('{}'::jsonb @> new.metadata)) EXECUTE FUNCTION public.claim_metadata_remove_row();


--
-- Name: assets delete_asset_when_amount_is_zero; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER delete_asset_when_amount_is_zero BEFORE INSERT OR UPDATE OF amount ON public.assets FOR EACH ROW WHEN ((new.amount = 0)) EXECUTE FUNCTION public.delete_asset_by_id();


--
-- Name: claim_metadata metadata_limitation; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER metadata_limitation BEFORE UPDATE OF metadata ON public.claim_metadata FOR EACH ROW WHEN ((NOT public.metadata_limitation(new.metadata))) EXECUTE FUNCTION public.metadata_limitation_trigger();


--
-- Name: claim_metadata strip_nulls_after_insert; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER strip_nulls_after_insert AFTER INSERT ON public.claim_metadata FOR EACH ROW EXECUTE FUNCTION public.claim_metadata_strip_nulls();


--
-- Name: access_tokens access_tokens_grant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.access_tokens
    ADD CONSTRAINT access_tokens_grant_id_fkey FOREIGN KEY (grant_id) REFERENCES public.grants(id) ON UPDATE CASCADE ON DELETE CASCADE;


--
-- Name: assets assets_money_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assets
    ADD CONSTRAINT assets_money_id_fkey FOREIGN KEY (currency_id) REFERENCES public.currencies(id);


--
-- Name: assets assets_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assets
    ADD CONSTRAINT assets_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: authorization_codes authorization_codes_application_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.authorization_codes
    ADD CONSTRAINT authorization_codes_application_id_fkey FOREIGN KEY (application_id) REFERENCES public.applications(id) ON UPDATE CASCADE ON DELETE CASCADE;


--
-- Name: claim_metadata claim_metadata_claim_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata
    ADD CONSTRAINT claim_metadata_claim_id_fkey FOREIGN KEY (claim_id) REFERENCES public.claims(id) ON DELETE CASCADE;


--
-- Name: claim_metadata claim_metadata_claimant_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata
    ADD CONSTRAINT claim_metadata_claimant_user_id_fkey FOREIGN KEY (claimant_user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: claim_metadata claim_metadata_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata
    ADD CONSTRAINT claim_metadata_fk FOREIGN KEY (claim_id, claimant_user_id, payer_user_id) REFERENCES public.claims(id, claimant_user_id, payer_user_id) ON DELETE CASCADE;


--
-- Name: claim_metadata claim_metadata_owner_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata
    ADD CONSTRAINT claim_metadata_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: claim_metadata claim_metadata_payer_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claim_metadata
    ADD CONSTRAINT claim_metadata_payer_user_id_fkey FOREIGN KEY (payer_user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: claims claims_claimant_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claims
    ADD CONSTRAINT claims_claimant_user_id_fkey FOREIGN KEY (claimant_user_id) REFERENCES public.users(id);


--
-- Name: claims claims_money_info_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claims
    ADD CONSTRAINT claims_money_info_id_fkey FOREIGN KEY (currency_id) REFERENCES public.currencies(id);


--
-- Name: claims claims_payer_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.claims
    ADD CONSTRAINT claims_payer_user_id_fkey FOREIGN KEY (payer_user_id) REFERENCES public.users(id);


--
-- Name: discord_users discord_users_discord_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_users
    ADD CONSTRAINT discord_users_discord_user_id_fkey FOREIGN KEY (discord_user_id) REFERENCES public.users(discord_id);


--
-- Name: grant_scopes grant_scopes_grant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.grant_scopes
    ADD CONSTRAINT grant_scopes_grant_id_fkey FOREIGN KEY (grant_id) REFERENCES public.grants(id) ON UPDATE CASCADE ON DELETE CASCADE;


--
-- Name: grants grants_application_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.grants
    ADD CONSTRAINT grants_application_id_fkey FOREIGN KEY (application_id) REFERENCES public.applications(id) ON UPDATE CASCADE ON DELETE CASCADE;


--
-- Name: currency_given_histories money_given_historys_money_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_given_histories
    ADD CONSTRAINT money_given_historys_money_id_fkey FOREIGN KEY (currency_id) REFERENCES public.currencies(id);


--
-- Name: currency_given_histories money_given_historys_receiver_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_given_histories
    ADD CONSTRAINT money_given_historys_receiver_id_fkey FOREIGN KEY (receiver_id) REFERENCES public.users(id);


--
-- Name: currency_payment_histories money_payment_historys_money_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_payment_histories
    ADD CONSTRAINT money_payment_historys_money_id_fkey FOREIGN KEY (currency_id) REFERENCES public.currencies(id);


--
-- Name: currency_payment_histories money_payment_historys_receiver_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_payment_histories
    ADD CONSTRAINT money_payment_historys_receiver_id_fkey FOREIGN KEY (receiver_id) REFERENCES public.users(id);


--
-- Name: currency_payment_histories money_payment_historys_sender_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.currency_payment_histories
    ADD CONSTRAINT money_payment_historys_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public.users(id);


--
-- Name: payments_idempotency payments_idempotency_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payments_idempotency
    ADD CONSTRAINT payments_idempotency_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: redirect_uris redirect_uris_application_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redirect_uris
    ADD CONSTRAINT redirect_uris_application_id_fkey FOREIGN KEY (application_id) REFERENCES public.applications(id) ON DELETE CASCADE;


--
-- Name: refresh_tokens refresh_tokens_grant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.refresh_tokens
    ADD CONSTRAINT refresh_tokens_grant_id_fkey FOREIGN KEY (grant_id) REFERENCES public.grants(id) ON UPDATE CASCADE ON DELETE CASCADE;


--
-- Name: user_access_tokens user_access_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_access_tokens
    ADD CONSTRAINT user_access_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: users users_application_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_application_id_fkey FOREIGN KEY (application_id) REFERENCES public.applications(id);


--
-- PostgreSQL database dump complete
--

\unrestrict h3eRyUFGfygQGVa6Li0DdQFLxhgN5fbzSjyigIPOZyvRYuYt4X1eyYH7OBaFzfa

