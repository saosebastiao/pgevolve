-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.address AS (street text, city text);
COMMENT ON TYPE app.address IS 'Postal address composite type';
