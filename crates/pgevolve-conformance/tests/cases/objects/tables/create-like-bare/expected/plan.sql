-- @pgevolve plan id=e01afcf6cdf4a3b7 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.clone
CREATE TABLE app.clone (
    id bigint NOT NULL,
    name text
);
COMMIT;

