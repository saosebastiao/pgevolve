-- @pgevolve schema=app
CREATE TABLE app.users (id bigint PRIMARY KEY, name text, active boolean);
CREATE VIEW active_users AS SELECT id, name FROM app.users WHERE active = true;
