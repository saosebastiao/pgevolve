-- @pgevolve plan id=ae5ffdc9dde356d7 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.logs
CREATE TABLE app.logs (
    id bigint NOT NULL,
    source text NOT NULL,
    message text NOT NULL
) PARTITION BY LIST (source);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.logs_app
CREATE TABLE app.logs_app PARTITION OF app.logs FOR VALUES IN ('app');
-- @pgevolve step=3 kind=create_table destructive=false targets=app.logs_other
CREATE TABLE app.logs_other PARTITION OF app.logs DEFAULT;
COMMIT;

