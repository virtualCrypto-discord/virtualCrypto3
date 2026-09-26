-- Replace persistent logins with short-lived, request-bound OAuth authorizations.
DROP TABLE browser_sessions;
CREATE TABLE browser_authorizations (
    id uuid PRIMARY KEY,
    browser_secret uuid NOT NULL,
    request jsonb NOT NULL,
    phase text NOT NULL DEFAULT 'discord' CHECK (phase IN ('discord', 'verifying', 'consent')),
    account_id bigint REFERENCES users(id) ON DELETE CASCADE,
    expires timestamptz NOT NULL DEFAULT (now() + interval '10 minutes'),
    CHECK ((phase = 'consent') = (account_id IS NOT NULL))
);
CREATE INDEX browser_authorizations_expires_index ON browser_authorizations (expires);
CREATE INDEX browser_authorizations_account_id_index ON browser_authorizations (account_id);
