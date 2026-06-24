-- @pgevolve plan id=6c00ec014ca8f89a version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_view destructive=false targets=app.annotated_view
COMMENT ON VIEW app.annotated_view IS 'Product catalogue view';
-- @pgevolve step=2 kind=comment_on_view destructive=false targets=app.annotated_view
COMMENT ON COLUMN app.annotated_view.name IS 'Product display name';
COMMIT;

