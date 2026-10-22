CREATE TYPE token_endpoint_auth_method AS ENUM ('none', 'client_secret_basic');

-- Existing applications retain their behavior. The API chooses the default for
-- new registrations from the configured activation time, not this DB default.
ALTER TABLE applications ADD COLUMN token_endpoint_auth_method
    token_endpoint_auth_method NOT NULL DEFAULT 'none';
