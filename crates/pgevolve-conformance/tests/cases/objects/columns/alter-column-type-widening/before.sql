-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
    id    integer NOT NULL,
    count integer NOT NULL,
    CONSTRAINT events_pkey PRIMARY KEY (id)
);
