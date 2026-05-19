-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.priority AS ENUM ('low', 'medium', 'high');
