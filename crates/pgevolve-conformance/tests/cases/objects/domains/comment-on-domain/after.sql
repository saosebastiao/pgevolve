-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.username AS text;
COMMENT ON DOMAIN app.username IS 'Validated username string';
