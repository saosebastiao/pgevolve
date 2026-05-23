-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (id bigint, author text);
CREATE POLICY only_authors ON app.docs AS RESTRICTIVE FOR INSERT WITH CHECK (author = current_user);
