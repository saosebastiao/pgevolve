-- @pgevolve plan id=a874e9ac5ae819fd version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.live
CREATE VIEW app.live (id, active) AS
SELECT id, active FROM app.t WHERE active = true
WITH CASCADED CHECK OPTION;
COMMIT;

