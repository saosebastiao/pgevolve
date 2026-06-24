-- @pgevolve plan id=e529b873282d942d version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_view_set_check_option destructive=false targets=app.live
CREATE OR REPLACE VIEW app.live (id, active) AS
SELECT id, active FROM app.t WHERE active = true
WITH CASCADED CHECK OPTION;
COMMIT;

