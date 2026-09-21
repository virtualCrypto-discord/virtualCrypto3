-- Personal grants: an ask put to a person rather than to a guild.
--
-- The grant this schema had is a guild's: `grants.guild_id` is where the
-- application may issue from, `grant_requests.guild_id` is who is being asked,
-- and both are NOT NULL. What this adds is the other target — a Discord user,
-- whose own account the application may act as once they say yes — as a second
-- column with exactly one of the two set. `docs/personal-grants.md` is the
-- design; this is the shape it needs.
--
-- Additive and non-breaking the way 0002 and 0004 are: the Elixir neither knows
-- nor reads either column, a row written by it has `discord_id` NULL and is
-- still a guild's, and the constraint below is a rule it cannot violate.
--
-- `ALTER TYPE ... ADD VALUE` is again the one statement that needs PostgreSQL 12,
-- and again it is not used in the same transaction: nothing here writes a scope.

ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.read';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.pay';
ALTER TYPE public.virtual_crypto_scope_type ADD VALUE IF NOT EXISTS 'vc.claim';

ALTER TABLE public.grants
    ADD COLUMN IF NOT EXISTS discord_id bigint;

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grants_one_target') THEN
    ALTER TABLE public.grants
        ADD CONSTRAINT grants_one_target
            CHECK ((guild_id IS NULL) <> (discord_id IS NULL)) NOT VALID;
  END IF;
END
$migration$;;

-- The pair a grant is unique by, for whichever target it names. `grants` already
-- has a unique index on (application_id, guild_id), which stays: it is the
-- Elixir's own, and this one subsumes it for guild rows without replacing it.
--
-- A guild id and a user id are both Discord snowflakes and Discord never issues
-- one of one kind and one of the other, so the two are one column space here and
-- in `grant_requests` below — which is what lets a single `ON CONFLICT` serve
-- both, rather than an upsert whose target is decided by a branch.
CREATE UNIQUE INDEX IF NOT EXISTS grants_application_id_target_index
    ON public.grants USING btree (application_id, COALESCE(guild_id, discord_id));

ALTER TABLE public.grant_requests
    ADD COLUMN IF NOT EXISTS discord_id bigint;

ALTER TABLE public.grant_requests
    ALTER COLUMN guild_id DROP NOT NULL;

DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'grant_requests_one_target') THEN
    ALTER TABLE public.grant_requests
        ADD CONSTRAINT grant_requests_one_target
            CHECK ((guild_id IS NULL) <> (discord_id IS NULL)) NOT VALID;
  END IF;
END
$migration$;;

-- The two indexes that named `guild_id` are replaced by the same pair over the
-- target: one pending ask per application and target, and one pending code per
-- target. `grant_requests_guild_id_index` stays — it is a lookup by guild, and a
-- personal row has none — and gains its counterpart for a person.
DROP INDEX IF EXISTS public.grant_requests_pending_index;
DROP INDEX IF EXISTS public.grant_requests_user_code_index;

CREATE UNIQUE INDEX IF NOT EXISTS grant_requests_pending_target_index
    ON public.grant_requests USING btree (application_id, COALESCE(guild_id, discord_id))
    WHERE status = 'pending';

CREATE UNIQUE INDEX IF NOT EXISTS grant_requests_user_code_target_index
    ON public.grant_requests USING btree (COALESCE(guild_id, discord_id), user_code)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS grant_requests_discord_id_index
    ON public.grant_requests USING btree (discord_id);
