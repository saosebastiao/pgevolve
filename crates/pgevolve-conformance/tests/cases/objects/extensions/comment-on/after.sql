-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE EXTENSION pgcrypto;
COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';
