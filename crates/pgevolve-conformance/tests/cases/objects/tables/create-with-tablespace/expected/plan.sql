-- @pgevolve plan id=13e912a3a8494bea version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.events
CREATE TABLE app.events (
    id bigint NOT NULL,
    payload text NOT NULL,
    CONSTRAINT events_pkey PRIMARY KEY (id)
) TABLESPACE ts_fast;
COMMIT;

