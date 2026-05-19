-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.positive_int AS integer
  NOT NULL
  DEFAULT 1
  CONSTRAINT positive CHECK (VALUE > 0);
