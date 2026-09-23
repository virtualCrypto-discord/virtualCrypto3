-- What a person has chosen not to see: a currency, or somebody whose actions would otherwise
-- appear in their lists.
--
-- Nothing here forbids anything. No money is stopped and no command refuses: what a row says is
-- which rows the claim and contract lists leave out for the person who wrote it, which is why
-- the pair is (who is looking, what they are not looking at) rather than a statement about the
-- target. Nobody else's screens change, and nobody is muted by being named — only the person who
-- set a mute is filtered by it.
--
-- No time column: a mute is a preference rather than a penalty, and it lasts until the person
-- who set it removes it.
--
-- An addition rather than a port: the Elixir has no mute, no column that could hold one, and
-- nothing that filters a list by its reader. It is ours alone, and the Elixir reading this
-- database sees a table it does not know and does not care about.
CREATE TABLE public.mutes (
    user_id integer NOT NULL,
    currency_id bigint,
    muted_user_id integer,
    inserted_at timestamptz NOT NULL,
    -- Exactly one of the two, which is what a row is about.
    CONSTRAINT mutes_one_target CHECK ((currency_id IS NULL) <> (muted_user_id IS NULL))
);

ALTER TABLE ONLY public.mutes
    ADD CONSTRAINT mutes_user_id_fkey FOREIGN KEY (user_id)
        REFERENCES public.users(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY public.mutes
    ADD CONSTRAINT mutes_currency_id_fkey FOREIGN KEY (currency_id)
        REFERENCES public.currencies(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY public.mutes
    ADD CONSTRAINT mutes_muted_user_id_fkey FOREIGN KEY (muted_user_id)
        REFERENCES public.users(id) ON UPDATE CASCADE ON DELETE CASCADE;

-- One mute per target: muting twice is the mute that is already there, and the unmute has one
-- row to delete. Two partial indexes rather than one constraint, because the target is one of
-- two columns and a plain UNIQUE counts the NULLs as distinct — which would let the second mute
-- of the same currency through.
CREATE UNIQUE INDEX mutes_user_currency_index
    ON public.mutes USING btree (user_id, currency_id) WHERE currency_id IS NOT NULL;

CREATE UNIQUE INDEX mutes_user_user_index
    ON public.mutes USING btree (user_id, muted_user_id) WHERE muted_user_id IS NOT NULL;
