-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE COLLATION app.ci (provider = icu, locale = 'und', deterministic = false);
