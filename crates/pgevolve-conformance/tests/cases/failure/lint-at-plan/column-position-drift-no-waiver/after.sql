-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id      bigint PRIMARY KEY,
    name    text   NOT NULL,
    email   text   NOT NULL
);
