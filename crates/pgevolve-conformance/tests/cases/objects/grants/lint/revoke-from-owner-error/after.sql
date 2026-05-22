-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
ALTER TABLE app.t OWNER TO app_owner;
