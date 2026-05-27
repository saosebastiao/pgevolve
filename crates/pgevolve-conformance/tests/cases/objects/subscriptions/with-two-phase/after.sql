CREATE SCHEMA app;
CREATE PUBLICATION p FOR ALL TABLES;
CREATE SUBSCRIPTION s
    CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}'
    PUBLICATION p
    WITH (two_phase = true);
