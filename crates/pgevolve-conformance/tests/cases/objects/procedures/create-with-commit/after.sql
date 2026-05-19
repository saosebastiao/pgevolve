-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.jobs (
  id bigint NOT NULL,
  status text,
  CONSTRAINT jobs_pkey PRIMARY KEY (id)
);
CREATE PROCEDURE app.batch_insert()
    LANGUAGE plpgsql
AS $$
DECLARE
  i integer;
BEGIN
  FOR i IN 1..10 LOOP
    INSERT INTO app.jobs (id, status) VALUES (i, 'pending');
    COMMIT;
  END LOOP;
END
$$;
