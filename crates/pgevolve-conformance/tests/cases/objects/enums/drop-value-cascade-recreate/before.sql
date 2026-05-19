-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.state AS ENUM ('draft', 'published', 'archived');
