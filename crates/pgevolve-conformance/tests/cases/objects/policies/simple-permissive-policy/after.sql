-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (id bigint, author text);
CREATE POLICY author_only ON app.docs USING (author = current_user);
