CREATE SCHEMA app;
CREATE SUBSCRIPTION s
    CONNECTION 'host=x dbname=app user=repl password=hunter2'
    PUBLICATION p;
