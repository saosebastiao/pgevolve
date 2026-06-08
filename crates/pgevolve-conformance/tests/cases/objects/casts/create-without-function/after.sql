-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.user_id AS integer;
CREATE DOMAIN app.account_id AS integer;
CREATE CAST (app.user_id AS app.account_id) WITHOUT FUNCTION;
