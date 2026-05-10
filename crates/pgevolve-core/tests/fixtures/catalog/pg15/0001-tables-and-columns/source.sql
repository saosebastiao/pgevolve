CREATE SCHEMA app;

CREATE TABLE app.types_zoo (
    id          bigint PRIMARY KEY,
    flag        boolean NOT NULL DEFAULT false,
    s_int       smallint,
    big         bigint,
    amt         numeric(10,2) NOT NULL,
    note        text,
    label       varchar(50) NOT NULL,
    fixed       char(8),
    body        bytea,
    born        date,
    when_t      time(3) without time zone,
    seen_at     timestamp with time zone NOT NULL DEFAULT now(),
    tag         uuid,
    payload     jsonb,
    addr_v4     inet,
    digits      integer[]
);
