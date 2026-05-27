-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint PRIMARY KEY, name text);
CREATE STATISTICS app.t_lower ON (lower(name)) FROM app.t;
