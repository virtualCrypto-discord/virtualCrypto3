-- Active browser sessions make logout revoke all copies of the cookie.
CREATE TABLE browser_sessions (
    id uuid PRIMARY KEY,
    user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires timestamptz NOT NULL
);
CREATE INDEX browser_sessions_expires_index ON browser_sessions (expires);
CREATE INDEX browser_sessions_user_id_index ON browser_sessions (user_id);
