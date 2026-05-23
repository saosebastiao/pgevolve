-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (id bigint);
ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
