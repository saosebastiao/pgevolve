-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (id bigint);
CREATE POLICY p ON app.docs FOR INSERT WITH CHECK (true);
