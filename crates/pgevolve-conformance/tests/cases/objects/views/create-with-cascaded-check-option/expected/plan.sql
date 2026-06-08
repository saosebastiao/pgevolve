-- @pgevolve plan id=3703d6838db4f8a3 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.live
CREATE VIEW app.live (id, active) AS
SELECT id, active FROM app.t WHERE active = true
WITH CASCADED CHECK OPTION;
COMMIT;

