-- @pgevolve plan id=40f5604f9d0d8c27 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.run_report
CREATE OR REPLACE FUNCTION app.run_report()
    RETURNS void
    LANGUAGE plpgsql
AS $pgevolve$DECLARE -- @pgevolve dep: app.reports
v_sql text; BEGIN v_sql := 'SELECT count(*) FROM app.reports'; EXECUTE v_sql; END$pgevolve$;
COMMIT;

