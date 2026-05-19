-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.status AS ENUM ('active', 'inactive', 'pending');
