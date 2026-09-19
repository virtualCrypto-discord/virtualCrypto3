-- Contracts: an application operating a user's currency, on the parties' approval.
--
-- The Elixir has three tables that were meant to be this — `contracts`
-- (`intermediary_id` → applications), `contractors` (a contract's users) and
-- `deposit_agreements` (a user's amount per currency, deposited and executed) —
-- and `users.contract_id` beside them. Nothing ever wrote any of them: its
-- `/contract/:id` page is a mockup with no endpoint behind it, and both tables
-- and column are empty. Two of the three also carry foreign keys that point at
-- the wrong tables (`deposit_agreements.contractor_id` → contracts rather than
-- contractors, `currency_id` → users rather than currencies), so the sketch was
-- never exercised either.
--
-- They are **replaced rather than grown**: these are the tables this service
-- writes, with the shape the feature needs, and a database that has the sketch
-- in it — the database this service shares with the Elixir — loses it here. That
-- is why the drops come first and are `IF EXISTS`: a database that never had the
-- sketch and one that did end up with the same schema, which is what the tests
-- and the deployment need.

-- `ALTER TYPE ... ADD VALUE` is the one statement here that a server must be
-- PostgreSQL 12 for, and it is not used in the same transaction: the tables
-- below name no scope.
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.contract';

-- `users` first: its foreign key is what keeps `contracts` from being dropped,
-- and the column goes with it rather than being left behind — a fresh database
-- has neither, and two schemas that differ by a column nobody writes is the kind
-- of difference that costs an afternoon later.
ALTER TABLE public.users DROP CONSTRAINT IF EXISTS users_contract_id_fkey;
ALTER TABLE public.users DROP COLUMN IF EXISTS contract_id;

DROP TABLE IF EXISTS public.deposit_agreements;
DROP TABLE IF EXISTS public.contractors;
DROP TABLE IF EXISTS public.contracts;

CREATE SEQUENCE IF NOT EXISTS public.contracts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE SEQUENCE IF NOT EXISTS public.contract_parties_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- One application's proposal to operate one currency. `expires_at` is the end of
-- a temporary contract; NULL is a permanent one, which is the kind a party may
-- withdraw from at any time. `receiver_discord_id` is the only Discord user the
-- locked money may be paid to, or NULL when the application names the receiver
-- payment by payment.
CREATE TABLE IF NOT EXISTS public.contracts (
    id bigint NOT NULL,
    application_id bigint NOT NULL,
    currency_id bigint NOT NULL,
    receiver_discord_id bigint,
    expires_at timestamp(0) without time zone,
    status character varying(255) NOT NULL DEFAULT 'pending',
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    CONSTRAINT contracts_status_is_known
        CHECK (status IN ('pending', 'active', 'canceled'))
);

ALTER SEQUENCE public.contracts_id_seq OWNED BY public.contracts.id;

ALTER TABLE ONLY public.contracts
    ALTER COLUMN id SET DEFAULT nextval('public.contracts_id_seq'::regclass);

-- One user the contract names, the amount that user locks by approving, and what
-- is left of it. `remaining` is the escrow: a party's locked money is what the
-- application has not spent of their approval yet, and the sum over the contract
-- is what it may still spend.
--
-- The party is a **discord id**, not an account: a contract may name someone who
-- has never used this service, and the account is created when they log in to
-- answer. `users_discord_id_index` is unique, so the join to an account is 1:1
-- once one exists.
CREATE TABLE IF NOT EXISTS public.contract_parties (
    id bigint NOT NULL,
    contract_id bigint NOT NULL,
    discord_id bigint NOT NULL,
    amount bigint NOT NULL,
    remaining bigint NOT NULL DEFAULT 0,
    status character varying(255) NOT NULL DEFAULT 'pending',
    inserted_at timestamp(0) without time zone NOT NULL,
    updated_at timestamp(0) without time zone NOT NULL,
    CONSTRAINT contract_parties_amount_is_positive CHECK (amount > 0),
    CONSTRAINT contract_parties_remaining_is_not_negative CHECK (remaining >= 0),
    CONSTRAINT contract_parties_status_is_known
        CHECK (status IN ('pending', 'approved', 'refused', 'withdrawn'))
);

ALTER SEQUENCE public.contract_parties_id_seq OWNED BY public.contract_parties.id;

ALTER TABLE ONLY public.contract_parties
    ALTER COLUMN id SET DEFAULT nextval('public.contract_parties_id_seq'::regclass);

CREATE UNIQUE INDEX IF NOT EXISTS contract_parties_contract_id_discord_id_index
    ON public.contract_parties USING btree (contract_id, discord_id);

CREATE INDEX IF NOT EXISTS contract_parties_discord_id_index
    ON public.contract_parties USING btree (discord_id);

CREATE INDEX IF NOT EXISTS contracts_application_id_index
    ON public.contracts USING btree (application_id);

CREATE INDEX IF NOT EXISTS contracts_currency_id_index
    ON public.contracts USING btree (currency_id);

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'contracts_pkey'
      AND conrelid = 'public.contracts'::regclass) THEN
    ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_pkey PRIMARY KEY (id);
  END IF;
END
$migration$;;

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'contract_parties_pkey'
      AND conrelid = 'public.contract_parties'::regclass) THEN
    ALTER TABLE ONLY public.contract_parties
    ADD CONSTRAINT contract_parties_pkey PRIMARY KEY (id);
  END IF;
END
$migration$;;

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'contracts_application_id_fkey'
      AND conrelid = 'public.contracts'::regclass) THEN
    ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_application_id_fkey FOREIGN KEY (application_id)
        REFERENCES public.applications(id) ON UPDATE CASCADE ON DELETE CASCADE;
  END IF;
END
$migration$;;

-- No cascade: a currency with a contract hanging off it is a currency whose
-- deletion has to say what happens to the locked money, rather than one that
-- silently takes other people's approvals with it.
DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'contracts_currency_id_fkey'
      AND conrelid = 'public.contracts'::regclass) THEN
    ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_currency_id_fkey FOREIGN KEY (currency_id)
        REFERENCES public.currencies(id);
  END IF;
END
$migration$;;

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'contract_parties_contract_id_fkey'
      AND conrelid = 'public.contract_parties'::regclass) THEN
    ALTER TABLE ONLY public.contract_parties
    ADD CONSTRAINT contract_parties_contract_id_fkey FOREIGN KEY (contract_id)
        REFERENCES public.contracts(id) ON UPDATE CASCADE ON DELETE CASCADE;
  END IF;
END
$migration$;;
