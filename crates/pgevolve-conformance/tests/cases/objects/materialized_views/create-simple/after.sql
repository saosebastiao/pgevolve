-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
  id bigint NOT NULL,
  user_id bigint NOT NULL,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
CREATE MATERIALIZED VIEW app.user_stats AS
  SELECT user_id, count(*) AS event_count
  FROM app.events
  GROUP BY user_id;
