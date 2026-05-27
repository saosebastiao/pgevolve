CREATE SCHEMA app;
CREATE PUBLICATION p FOR ALL TABLES;
CREATE SUBSCRIPTION s
    CONNECTION 'host=primary.example.com dbname=app user=repl password=${PWD_A}'
    PUBLICATION p;
