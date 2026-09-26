CREATE TABLE security_webhook_queue (
    id bigserial PRIMARY KEY,
    signal text NOT NULL,
    body jsonb NOT NULL,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    inserted_at timestamptz NOT NULL DEFAULT now(),
    failed_at timestamptz,
    last_status smallint
);

CREATE INDEX security_webhook_queue_pending_index
    ON security_webhook_queue (id) WHERE failed_at IS NULL;
CREATE INDEX security_webhook_queue_failed_index
    ON security_webhook_queue (failed_at) WHERE failed_at IS NOT NULL;
