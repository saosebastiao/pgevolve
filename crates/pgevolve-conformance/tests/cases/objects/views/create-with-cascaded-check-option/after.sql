-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint PRIMARY KEY, active boolean);
CREATE VIEW app.live AS SELECT id, active FROM app.t WHERE active = true
    WITH CASCADED CHECK OPTION;
