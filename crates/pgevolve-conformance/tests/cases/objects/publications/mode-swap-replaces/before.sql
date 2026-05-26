-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.x (id bigint PRIMARY KEY);
CREATE PUBLICATION main FOR ALL TABLES;
