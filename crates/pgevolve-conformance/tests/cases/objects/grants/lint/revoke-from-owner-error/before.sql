-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
ALTER TABLE app.t OWNER TO app_owner;
GRANT SELECT ON TABLE app.t TO app_owner;
