-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.accounts (
    id      bigint NOT NULL,
    country text   NOT NULL,
    balance numeric NOT NULL
) PARTITION BY LIST (country);
