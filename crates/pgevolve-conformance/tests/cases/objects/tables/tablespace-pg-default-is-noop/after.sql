-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint NOT NULL,
    body text   NOT NULL,
    CONSTRAINT docs_pkey PRIMARY KEY (id)
) TABLESPACE pg_default;
