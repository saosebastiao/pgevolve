-- @pgevolve plan id=0d565b377e22047a version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.status
CREATE TYPE app.status AS ENUM ('open', 'closed');
-- @pgevolve step=2 kind=create_table destructive=false targets=app.events
CREATE TABLE app.events (
    id bigint NOT NULL,
    current_status app.status NOT NULL,
    CONSTRAINT events_pkey PRIMARY KEY (id)
);
COMMIT;

