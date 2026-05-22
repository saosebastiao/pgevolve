-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE SEQUENCE app.id_seq;
GRANT USAGE ON SEQUENCE app.id_seq TO readers;
