-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.base (
    id   bigint NOT NULL,
    name text,
    CONSTRAINT base_pkey PRIMARY KEY (id)
);
