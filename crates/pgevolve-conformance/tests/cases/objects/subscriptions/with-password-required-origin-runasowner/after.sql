CREATE SCHEMA app;
CREATE PUBLICATION p FOR ALL TABLES;
CREATE SUBSCRIPTION s
    CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}'
    PUBLICATION p
    WITH (password_required = true, origin = none, run_as_owner = true);
