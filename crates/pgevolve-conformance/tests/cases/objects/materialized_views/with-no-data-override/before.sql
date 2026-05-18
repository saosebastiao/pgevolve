-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
  id bigint NOT NULL,
  event_date date NOT NULL,
  metric_value numeric NOT NULL,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
