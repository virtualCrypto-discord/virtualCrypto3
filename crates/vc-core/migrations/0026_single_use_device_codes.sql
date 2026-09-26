-- Retain approval history while preventing a second device-code exchange.
ALTER TABLE grant_requests DROP CONSTRAINT grant_requests_status_is_known;
ALTER TABLE grant_requests ADD CONSTRAINT grant_requests_status_is_known
    CHECK (status IN ('pending', 'approved', 'exchanged'));

-- Previously issued tokens prove that these requests have already been exchanged.
UPDATE grant_requests r SET status = 'exchanged'
WHERE r.status = 'approved' AND (
    EXISTS (SELECT 1 FROM access_tokens t WHERE t.grant_id = r.grant_id)
    OR EXISTS (SELECT 1 FROM refresh_tokens t WHERE t.grant_id = r.grant_id)
);
