-- Small values the app keeps between runs, e.g. what the library was last
-- grouped with (a scan that finds nothing changed skips regrouping).
CREATE TABLE app_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
