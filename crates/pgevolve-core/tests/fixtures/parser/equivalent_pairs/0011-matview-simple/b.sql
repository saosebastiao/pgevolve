-- @pgevolve schema=app
CREATE MATERIALIZED VIEW event_summary AS SELECT count(*) AS total FROM app.events WITH NO DATA;
