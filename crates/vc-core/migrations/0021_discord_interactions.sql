-- Claim an interaction before dispatch. Keep the claim even if the process dies:
-- handlers commit their own transactions, so an unfinished response does not
-- prove that it is safe to execute the interaction again.
CREATE TABLE discord_interactions (
    id text PRIMARY KEY,
    status integer,
    content_type text,
    body bytea,
    inserted_at timestamp without time zone NOT NULL DEFAULT (now() AT TIME ZONE 'utc'),
    CHECK (status IS NULL OR (status BETWEEN 100 AND 599 AND body IS NOT NULL))
);
