-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.items (
    id       bigint NOT NULL,
    priority integer,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
