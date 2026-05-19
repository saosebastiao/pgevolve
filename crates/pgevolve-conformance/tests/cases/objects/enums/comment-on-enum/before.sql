-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.role AS ENUM ('admin', 'editor', 'viewer');
