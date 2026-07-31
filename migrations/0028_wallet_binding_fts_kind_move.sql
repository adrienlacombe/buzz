-- FORK-LOCAL (adrienlacombe/buzz): re-point the NIP-SW search exclusion after the
-- wallet-binding kind moved 30178 -> 30900.
--
-- Upstream shipped KIND_TEAM_CATALOG = 30178 (block/buzz#3358) onto the integer
-- this fork's Starknet wallet binding already held. Per AGENTS.md the fork yields
-- the contested integer and moves into its reserved 30900-30999 block.
--
-- 0027 hardcoded `kind = 30178` into the search_tsv generated expression. That
-- migration has already run on live databases and will not re-run, so without
-- this follow-on two things break at once: wallet addresses at kind 30900 become
-- full-text indexed (the exact correlation 0027 exists to prevent), and upstream's
-- team-catalog events at 30178 are silently excluded from search instead.
--
-- Same capture-drop-re-add mechanism as 0014 and 0027: PostgreSQL cannot alter a
-- generated expression in place. Wrapping rather than replacing preserves the
-- fresh-install allowlist from 0008 and any brownfield expression from 0001 for
-- every other kind.
--
-- The rewrite below strips the old `kind = 30178` guard out of the captured
-- expression before re-wrapping with 30900, so applying this once is idempotent
-- in effect and does not leave a stale exclusion nested inside the new one.
DO $$
DECLARE
    existing_expression TEXT;
    unwrapped_expression TEXT;
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

    -- Peel off 0027's wrapper if it is present. Postgres normalises the stored
    -- expression, so match on the CASE arm rather than on 0027's exact source
    -- text. If the pattern does not match -- a database that never ran 0027, or
    -- an operator-managed expression -- fall through and wrap what is there.
    unwrapped_expression := regexp_replace(
        existing_expression,
        '^CASE WHEN \(?kind = 30178\)? THEN NULL::tsvector ELSE \((.*)\) END$',
        '\1',
        's'
    );

    ALTER TABLE events DROP COLUMN search_tsv;
    EXECUTE format(
        'ALTER TABLE events ADD COLUMN search_tsv TSVECTOR GENERATED ALWAYS AS (CASE WHEN kind = 30900 THEN NULL::tsvector ELSE (%s) END) STORED',
        unwrapped_expression
    );
    CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);
END $$;
