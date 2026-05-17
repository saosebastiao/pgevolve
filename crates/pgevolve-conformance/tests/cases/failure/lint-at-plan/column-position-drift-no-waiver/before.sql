-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id      bigint PRIMARY KEY,
    email   text   NOT NULL,
    name    text   NOT NULL
);
