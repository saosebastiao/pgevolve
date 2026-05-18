-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.data (
  id bigint NOT NULL,
  payload text,
  CONSTRAINT data_pkey PRIMARY KEY (id)
);
-- Enable security_invoker (PG 15+)
CREATE VIEW app.invoker_view WITH (security_invoker = true) AS
  SELECT id, payload FROM app.data;
