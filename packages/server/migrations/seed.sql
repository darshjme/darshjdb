-- DarshanDB: Seed data -- a "todo app" scenario
-- 3 users, 5 todos with references between them.
--
-- Value type discriminators (from schema.rs):
--   0 = String
--   1 = Integer
--   2 = Float
--   3 = Boolean
--   4 = Timestamp
--   5 = Reference (UUID pointing to another entity)
--   6 = Json

-- ── Allocate transaction ids ───────────────────────────────────────

-- tx 1: create users
-- tx 2: create todos

-- ── Users ──────────────────────────────────────────────────────────

-- User 1: Alice
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('a0000000-0000-0000-0000-000000000001', ':db/type',    '"users"',                 0, nextval('darshan_tx_seq')),
    ('a0000000-0000-0000-0000-000000000001', 'users/name',  '"Alice Johnson"',         0, currval('darshan_tx_seq')),
    ('a0000000-0000-0000-0000-000000000001', 'users/email', '"alice@example.com"',     0, currval('darshan_tx_seq'));

-- User 2: Bob
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('a0000000-0000-0000-0000-000000000002', ':db/type',    '"users"',                 0, nextval('darshan_tx_seq')),
    ('a0000000-0000-0000-0000-000000000002', 'users/name',  '"Bob Smith"',             0, currval('darshan_tx_seq')),
    ('a0000000-0000-0000-0000-000000000002', 'users/email', '"bob@example.com"',       0, currval('darshan_tx_seq'));

-- User 3: Carol
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('a0000000-0000-0000-0000-000000000003', ':db/type',    '"users"',                 0, nextval('darshan_tx_seq')),
    ('a0000000-0000-0000-0000-000000000003', 'users/name',  '"Carol Davis"',           0, currval('darshan_tx_seq')),
    ('a0000000-0000-0000-0000-000000000003', 'users/email', '"carol@example.com"',     0, currval('darshan_tx_seq'));

-- ── Todos ──────────────────────────────────────────────────────────

-- Todo 1: Buy groceries (Alice, done)
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('b0000000-0000-0000-0000-000000000001', ':db/type',      '"todos"',                                  0, nextval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000001', 'todos/title',   '"Buy groceries"',                          0, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000001', 'todos/done',    'true',                                     3, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000001', 'todos/userId',  '"a0000000-0000-0000-0000-000000000001"',   5, currval('darshan_tx_seq'));

-- Todo 2: Write docs (Alice, not done)
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('b0000000-0000-0000-0000-000000000002', ':db/type',      '"todos"',                                  0, nextval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000002', 'todos/title',   '"Write documentation"',                    0, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000002', 'todos/done',    'false',                                    3, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000002', 'todos/userId',  '"a0000000-0000-0000-0000-000000000001"',   5, currval('darshan_tx_seq'));

-- Todo 3: Review PRs (Bob, not done)
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('b0000000-0000-0000-0000-000000000003', ':db/type',      '"todos"',                                  0, nextval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000003', 'todos/title',   '"Review pull requests"',                   0, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000003', 'todos/done',    'false',                                    3, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000003', 'todos/userId',  '"a0000000-0000-0000-0000-000000000002"',   5, currval('darshan_tx_seq'));

-- Todo 4: Deploy v2 (Bob, done)
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('b0000000-0000-0000-0000-000000000004', ':db/type',      '"todos"',                                  0, nextval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000004', 'todos/title',   '"Deploy v2 to production"',                0, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000004', 'todos/done',    'true',                                     3, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000004', 'todos/userId',  '"a0000000-0000-0000-0000-000000000002"',   5, currval('darshan_tx_seq'));

-- Todo 5: Setup CI (Carol, not done)
INSERT INTO triples (entity_id, attribute, value, value_type, tx_id) VALUES
    ('b0000000-0000-0000-0000-000000000005', ':db/type',      '"todos"',                                  0, nextval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000005', 'todos/title',   '"Setup CI pipeline"',                      0, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000005', 'todos/done',    'false',                                    3, currval('darshan_tx_seq')),
    ('b0000000-0000-0000-0000-000000000005', 'todos/userId',  '"a0000000-0000-0000-0000-000000000003"',   5, currval('darshan_tx_seq'));
