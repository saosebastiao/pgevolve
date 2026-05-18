CREATE MATERIALIZED VIEW app.event_summary AS SELECT count(*) AS total FROM app.events WITH NO DATA;
