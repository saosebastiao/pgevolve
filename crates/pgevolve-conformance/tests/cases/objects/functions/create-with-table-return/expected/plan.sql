-- @pgevolve plan id=ce6398b532d5be6e version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.get_summary
CREATE OR REPLACE FUNCTION app.get_summary()
    RETURNS TABLE(id integer, label text)
    LANGUAGE sql STABLE
AS $pgevolve$SELECT $1, $2
  UNION ALL
  SELECT $3, $4$pgevolve$;
COMMIT;

