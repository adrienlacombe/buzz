-- NIP-SW kind:30178 carries a Starknet account address. Addresses are public
-- on-chain, but surfacing them in message search would let anyone enumerate
-- which members have bound wallets by searching for an address fragment, which
-- is a correlation the binding itself does not publish. Exclude the kind from
-- full-text search.
--
-- Wrap the current expression rather than replacing it. Migration 0008
-- deliberately gives only empty/fresh databases the positive allowlist, so a
-- populated database still runs the original exclusion-list expression from
-- 0001 — under which every kind not explicitly listed IS indexed. Replacing the
-- expression outright would silently change the search policy for every other
-- kind on brownfield installs; wrapping preserves it.
--
-- Same mechanism as 0014_push_lease_fts.sql. PostgreSQL cannot alter a generated
-- expression in place, so capture it, drop the column, and re-add it wrapped.
DO $$
DECLARE
    existing_expression TEXT;
BEGIN
    SELECT pg_get_expr(d.adbin, d.adrelid)
      INTO existing_expression
      FROM pg_attrdef d
      JOIN pg_attribute a
        ON a.attrelid = d.adrelid
       AND a.attnum = d.adnum
     WHERE d.adrelid = 'events'::regclass
       AND a.attname = 'search_tsv';

    IF existing_expression IS NULL THEN
        RAISE EXCEPTION 'events.search_tsv generated expression not found';
    END IF;

    ALTER TABLE events DROP COLUMN search_tsv;
    EXECUTE format(
        'ALTER TABLE events ADD COLUMN search_tsv TSVECTOR GENERATED ALWAYS AS (CASE WHEN kind = 30178 THEN NULL::tsvector ELSE (%s) END) STORED',
        existing_expression
    );
    CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);
END $$;
