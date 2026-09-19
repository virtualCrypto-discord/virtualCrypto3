-- The one thing a job cannot read off the domain: the day it last ran.
--
-- `reset_pool_amount` adds a day's allowance and remembers nothing, so the
-- schedule is the whole of what keeps it daily. A schedule held in a process is
-- paid again by the next restart, which is an allowance no day asked for.
CREATE TABLE public.job_runs (
    name character varying(255) NOT NULL,
    ran_on date NOT NULL,
    CONSTRAINT job_runs_pkey PRIMARY KEY (name)
);
