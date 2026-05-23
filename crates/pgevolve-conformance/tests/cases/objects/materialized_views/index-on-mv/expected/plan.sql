-- @pgevolve plan id=fc38ba55b8926f6f version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_index destructive=false targets=app.product_summary_category_idx,app.product_summary
CREATE INDEX product_summary_category_idx ON app.product_summary USING btree (category);
COMMIT;

