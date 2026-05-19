-- @pgevolve schema=app
CREATE TABLE app.log (n integer);
CREATE PROCEDURE batch_commit(batch_size integer)
    LANGUAGE plpgsql
    AS $$
    BEGIN
        FOR i IN 1..batch_size LOOP
            INSERT INTO app.log(n) VALUES (i);
            COMMIT;
        END LOOP;
    END
    $$;
