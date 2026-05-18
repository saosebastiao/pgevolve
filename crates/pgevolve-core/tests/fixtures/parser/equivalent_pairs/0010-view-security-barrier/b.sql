-- @pgevolve schema=app
CREATE TABLE app.users (id bigint PRIMARY KEY);
CREATE VIEW secure_v WITH (security_barrier = true) AS SELECT id FROM app.users;
