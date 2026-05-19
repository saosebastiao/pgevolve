-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.role AS ENUM ('admin', 'editor', 'viewer');
COMMENT ON TYPE app.role IS 'User roles for access control';
