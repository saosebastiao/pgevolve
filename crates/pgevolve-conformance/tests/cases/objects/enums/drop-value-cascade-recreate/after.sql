-- @pgevolve schema=app
CREATE SCHEMA app;
-- Remove 'published' — requires DROP TYPE … CASCADE + CREATE TYPE
CREATE TYPE app.state AS ENUM ('draft', 'archived');
