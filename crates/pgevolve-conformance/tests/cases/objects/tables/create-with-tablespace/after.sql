-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
    id      bigint NOT NULL,
    payload text   NOT NULL,
    CONSTRAINT events_pkey PRIMARY KEY (id)
) TABLESPACE ts_fast;
