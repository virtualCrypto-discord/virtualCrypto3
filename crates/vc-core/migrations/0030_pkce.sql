-- NULL preserves codes issued before PKCE support and optional legacy flows.
-- S256 is the only supported method; a non-NULL value records its challenge.
ALTER TABLE authorization_codes ADD COLUMN code_challenge text
    CHECK (code_challenge IS NULL OR code_challenge ~ '^[A-Za-z0-9_-]{43}$');
