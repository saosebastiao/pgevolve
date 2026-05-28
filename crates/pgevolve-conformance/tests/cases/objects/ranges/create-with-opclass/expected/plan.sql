-- @pgevolve plan id=3ddd77840a1bdd34 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.textpat_range
CREATE TYPE app.textpat_range AS RANGE (subtype = pg_catalog.text, subtype_opclass = pg_catalog.text_pattern_ops);
COMMIT;

