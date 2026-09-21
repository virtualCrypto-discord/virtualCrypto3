-- A credential for something that is not a browser: long-lived, named, and revocable from
-- Discord. The row is already the whole of how a JWT here is revoked, so nothing new is
-- needed to make such a token die — what is needed is a way to tell its row apart from the
-- hour-long one a session writes, and a name to revoke it by.
ALTER TABLE public.user_access_tokens ADD COLUMN name text;

-- Names are how a token is revoked, so two of them on one account could not be told apart.
CREATE UNIQUE INDEX user_access_tokens_name_index
    ON public.user_access_tokens (user_id, name);
