-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.accounts (
  id bigint NOT NULL,
  owner text,
  CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
CREATE VIEW app.secure_view WITH (security_barrier = false) AS
  SELECT id, owner FROM app.accounts;
