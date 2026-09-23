-- Device and v3 browser approvals are independent grants. Legacy scope-less/OIDC
-- authorization-code grants retain their application/guild slot and behavior.
ALTER TABLE grants ADD COLUMN independent boolean NOT NULL DEFAULT false;
DROP INDEX grants_application_id_target_index;
DROP INDEX grants_application_id_guild_id_index;
CREATE UNIQUE INDEX grants_application_id_guild_id_index
 ON grants(application_id, guild_id) WHERE NOT independent;
CREATE UNIQUE INDEX grants_application_id_target_index
 ON grants(application_id, COALESCE(guild_id, discord_id)) WHERE NOT independent;
ALTER TABLE grant_requests ADD COLUMN grant_id bigint REFERENCES grants(id) ON DELETE SET NULL;
-- Requests with different permissions must coexist. Exact retries are serialized
-- on the application row and reuse only an identical pending request.
DROP INDEX grant_requests_pending_target_index;
CREATE INDEX grant_requests_grant_id_index ON grant_requests(grant_id);
CREATE INDEX grant_requests_pending_application_index
 ON grant_requests(application_id) WHERE status = 'pending';
CREATE INDEX grants_independent_application_target_index
 ON grants(application_id, COALESCE(guild_id, discord_id)) WHERE independent;
