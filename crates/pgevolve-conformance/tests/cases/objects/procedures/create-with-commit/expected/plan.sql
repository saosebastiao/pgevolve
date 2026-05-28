-- @pgevolve plan id=c36044f429063075 version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=false
-- @pgevolve step=1 kind=create_or_replace_procedure destructive=false targets=app.batch_insert
CREATE OR REPLACE PROCEDURE app.batch_insert()
    LANGUAGE plpgsql
AS $pgevolve$DECLARE i integer; BEGIN FOR i IN 1..10 LOOP INSERT INTO app.jobs (id, status) VALUES (i, 'pending'); COMMIT; END LOOP; END$pgevolve$;

