-- @pgevolve plan id=d3a3e3ecc3dbed78 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.docs
CREATE TABLE app.docs (
    id bigint NOT NULL,
    body text COMPRESSION lz4,
    CONSTRAINT docs_pkey PRIMARY KEY (id)
);
-- @pgevolve step=2 kind=set_column_storage destructive=false targets=app.docs
ALTER TABLE app.docs ALTER COLUMN body SET STORAGE EXTERNAL;
COMMIT;

