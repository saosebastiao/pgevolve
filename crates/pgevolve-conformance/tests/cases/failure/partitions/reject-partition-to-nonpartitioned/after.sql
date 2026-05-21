-- @pgevolve schema=app
CREATE SCHEMA app;
-- Removing PARTITION BY from app.accounts is not supported in-place.
-- The differ emits UnsupportedDiff which aborts planning.
CREATE TABLE app.accounts (
    id      bigint NOT NULL,
    country text   NOT NULL,
    balance numeric NOT NULL
);
