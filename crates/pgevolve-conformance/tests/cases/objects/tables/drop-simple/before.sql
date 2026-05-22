-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.legacy (
    id   bigint NOT NULL,
    data text,
    CONSTRAINT legacy_pkey PRIMARY KEY (id)
);
