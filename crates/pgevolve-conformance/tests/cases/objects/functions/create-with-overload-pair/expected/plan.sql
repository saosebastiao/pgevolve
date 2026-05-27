-- @pgevolve plan id=7b5a7deaf54a1571 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.add
CREATE OR REPLACE FUNCTION app.add(a text, b text)
    RETURNS text
    LANGUAGE sql IMMUTABLE STRICT
AS $pgevolve$SELECT a || b$pgevolve$;
-- @pgevolve step=2 kind=create_or_replace_function destructive=false targets=app.add
CREATE OR REPLACE FUNCTION app.add(a integer, b integer)
    RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
AS $pgevolve$SELECT a + b$pgevolve$;
COMMIT;

