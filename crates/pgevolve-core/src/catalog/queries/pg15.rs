//! PG 15-specific query overrides.

/// Subscriptions for PG 15.
///
/// PG 15 added `subtwophasestate` and `subdisableonerr` but lacks:
///   - `subpasswordrequired`, `subrunasowner`, `suborigin` (PG 16+)
///   - `subfailover` (PG 17+)
///
/// `substream` is still `bool` in PG 15; the `::text` cast normalises it.
pub const SUBSCRIPTIONS_QUERY_PG15: &str = "\
    SELECT \
        s.oid::bigint AS oid, \
        s.subname::text AS name, \
        coalesce(a.rolname, '') AS owner, \
        s.subenabled AS enabled, \
        s.subconninfo::text AS connection, \
        coalesce(s.subslotname::text, '') AS slot_name, \
        s.subsynccommit::text AS synchronous_commit, \
        s.subpublications::text[] AS publications, \
        s.subbinary AS binary, \
        s.substream::text AS streaming, \
        s.subtwophasestate::text AS two_phase_state, \
        s.subdisableonerr AS disable_on_error, \
        NULL::bool AS password_required, \
        NULL::bool AS run_as_owner, \
        NULL::text AS origin, \
        NULL::bool AS failover, \
        coalesce(d.description, '') AS comment \
    FROM pg_subscription s \
    JOIN pg_authid a ON a.oid = s.subowner \
    LEFT JOIN pg_description d \
        ON d.classoid = 'pg_subscription'::regclass AND d.objoid = s.oid AND d.objsubid = 0 \
    ORDER BY s.subname";
