-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.data (
  id bigint NOT NULL,
  payload text,
  CONSTRAINT data_pkey PRIMARY KEY (id)
);
CREATE VIEW app.invoker_view WITH (security_invoker = false) AS
  SELECT id, payload FROM app.data;
