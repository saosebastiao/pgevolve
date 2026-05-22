-- @pgevolve plan id=eec49e4f5b68c4ae version=0.2.1 ruleset=1
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

