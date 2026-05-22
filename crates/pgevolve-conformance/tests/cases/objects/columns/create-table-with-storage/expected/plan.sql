-- @pgevolve plan id=49bf382ae3573647 version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.docs
CREATE TABLE app.docs (
    id bigint NOT NULL,
    body text STORAGE EXTERNAL COMPRESSION lz4,
    CONSTRAINT docs_pkey PRIMARY KEY (id)
);
COMMIT;

