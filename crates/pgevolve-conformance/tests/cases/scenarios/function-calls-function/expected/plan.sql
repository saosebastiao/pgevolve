-- @pgevolve plan id=51f258212fde7f75 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.base_value
CREATE OR REPLACE FUNCTION app.base_value()
    RETURNS integer
    LANGUAGE sql IMMUTABLE
AS $pgevolve$SELECT 10$pgevolve$;
-- @pgevolve step=2 kind=create_or_replace_function destructive=false targets=app.doubled_value
CREATE OR REPLACE FUNCTION app.doubled_value()
    RETURNS integer
    LANGUAGE plpgsql IMMUTABLE
AS $pgevolve$DECLARE -- @pgevolve dep: app.base_value
v integer; BEGIN SELECT app.base_value() INTO v; RETURN v * 2; END$pgevolve$;
COMMIT;

