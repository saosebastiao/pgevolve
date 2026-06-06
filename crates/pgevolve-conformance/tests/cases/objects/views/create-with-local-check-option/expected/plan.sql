-- @pgevolve plan id=d0ad3df69ccb5761 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.live
CREATE VIEW app.live (id, active) AS
SELECT id, active FROM app.t WHERE active = true
WITH LOCAL CHECK OPTION;
COMMIT;

