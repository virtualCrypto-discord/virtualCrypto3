CREATE TYPE pkce_policy AS ENUM ('optional', 'required');

-- Preserve existing applications; registration chooses its dated default.
ALTER TABLE applications ADD COLUMN pkce_policy pkce_policy NOT NULL DEFAULT 'optional';
