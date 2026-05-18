-- @pgevolve schema=app
CREATE VIEW active_users AS SELECT id, name FROM app.users WHERE active = true;
