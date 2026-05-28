-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.int_window AS RANGE (subtype = int4);
