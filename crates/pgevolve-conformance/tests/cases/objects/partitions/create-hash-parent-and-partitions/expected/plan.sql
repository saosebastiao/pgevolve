-- @pgevolve plan id=91bd2dfb54ea6990 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.events
CREATE TABLE app.events (
    id bigint NOT NULL,
    payload jsonb NOT NULL
) PARTITION BY HASH (id);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.events_p0
CREATE TABLE app.events_p0 PARTITION OF app.events FOR VALUES WITH (MODULUS 4, REMAINDER 0);
-- @pgevolve step=3 kind=create_table destructive=false targets=app.events_p1
CREATE TABLE app.events_p1 PARTITION OF app.events FOR VALUES WITH (MODULUS 4, REMAINDER 1);
-- @pgevolve step=4 kind=create_table destructive=false targets=app.events_p2
CREATE TABLE app.events_p2 PARTITION OF app.events FOR VALUES WITH (MODULUS 4, REMAINDER 2);
-- @pgevolve step=5 kind=create_table destructive=false targets=app.events_p3
CREATE TABLE app.events_p3 PARTITION OF app.events FOR VALUES WITH (MODULUS 4, REMAINDER 3);
COMMIT;

