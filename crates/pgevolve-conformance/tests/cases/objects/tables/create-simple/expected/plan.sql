-- @pgevolve plan id=9f7ca032eb473f7e version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.t
CREATE TABLE app.t (
    id bigint NOT NULL,
    CONSTRAINT t_pkey PRIMARY KEY (id)
);
COMMIT;

