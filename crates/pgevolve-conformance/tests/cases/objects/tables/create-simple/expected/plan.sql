-- @pgevolve plan id=6d1782fe9d7ce5ae version=0.4.3 ruleset=1
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

