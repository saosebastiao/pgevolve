CREATE TABLE app.users (id bigint PRIMARY KEY, name text);
CREATE VIEW app.v(user_id, user_name) AS SELECT id, name FROM app.users;
