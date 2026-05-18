-- @pgevolve schema=app
CREATE VIEW v(user_id, user_name) AS SELECT id, name FROM app.users;
