-- @pgevolve schema=app
CREATE TABLE app.events (id bigint PRIMARY KEY);
CREATE MATERIALIZED VIEW event_summary AS SELECT count(*) AS total FROM app.events WITH NO DATA;
