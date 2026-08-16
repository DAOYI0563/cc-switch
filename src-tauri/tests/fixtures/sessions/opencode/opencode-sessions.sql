PRAGMA foreign_keys = ON;

CREATE TABLE session (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    directory TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL
);

CREATE TABLE message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    data TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE TABLE part (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    data TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES session(id) ON DELETE CASCADE,
    FOREIGN KEY(message_id) REFERENCES message(id) ON DELETE CASCADE
);

INSERT INTO session (id, title, directory, time_created, time_updated)
VALUES (
    'ses_fixture_opencode_sqlite', 'Fixture SQLite Session',
    '/workspace/fixture-project', 1767229200000, 1767229205000
);

INSERT INTO message (id, session_id, time_created, data)
VALUES (
    'fixture-opencode-sqlite-message', 'ses_fixture_opencode_sqlite',
    1767229201000,
    '{"id":"fixture-opencode-sqlite-message","role":"user"}'
);

INSERT INTO part (id, session_id, message_id, time_created, data)
VALUES (
    'fixture-opencode-sqlite-part', 'ses_fixture_opencode_sqlite',
    'fixture-opencode-sqlite-message', 1767229202000,
    '{"id":"fixture-opencode-sqlite-part","type":"text","text":"Inspect the SQLite fixture contract"}'
);
