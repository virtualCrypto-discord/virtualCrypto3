-- The common /grant approve entry point must resolve a code to one pending ask,
-- regardless of whether its target is a person or a guild.
DROP INDEX IF EXISTS public.grant_requests_user_code_target_index;
CREATE UNIQUE INDEX grant_requests_pending_user_code_index
    ON public.grant_requests (user_code) WHERE status = 'pending';
