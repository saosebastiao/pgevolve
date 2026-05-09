CREATE SEQUENCE app.users_id_seq AS integer OWNED BY app.users.id;
CREATE TABLE app.users (
    id integer NOT NULL DEFAULT nextval('app.users_id_seq'::regclass) PRIMARY KEY
);
