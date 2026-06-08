-- @pgevolve plan id=d9dfdecdb9e01f24 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_function destructive=false targets=app.double
COMMENT ON FUNCTION app.double(integer) IS 'Returns twice the input value';
COMMIT;

