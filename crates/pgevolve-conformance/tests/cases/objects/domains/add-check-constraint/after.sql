-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.score AS integer
  CONSTRAINT valid_score CHECK (VALUE >= 0 AND VALUE <= 100);
