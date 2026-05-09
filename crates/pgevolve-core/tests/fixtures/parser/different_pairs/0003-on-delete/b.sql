CREATE TABLE app.parent (id integer PRIMARY KEY);
CREATE TABLE app.child (
    p_id integer REFERENCES app.parent (id) ON DELETE NO ACTION
);
