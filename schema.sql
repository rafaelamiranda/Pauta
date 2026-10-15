-- Conversas e mensagens, com índice de busca de texto completo (FTS5).

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  subject TEXT,
  overview TEXT,
  pending TEXT,
  status TEXT NOT NULL DEFAULT 'ended' CHECK(status IN ('active', 'ended')),
  started_at INTEGER NOT NULL,
  ended_at INTEGER
);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
  content TEXT NOT NULL,
  speaker TEXT CHECK(speaker IN ('ME', 'THEY')),
  timestamp INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);

-- unicode61 com remove_diacritics: "reuniao" encontra "reunião".
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  id UNINDEXED,
  session_id UNINDEXED,
  content,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, id, session_id, content)
  VALUES (new.rowid, new.id, new.session_id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
  DELETE FROM messages_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE OF content ON messages BEGIN
  UPDATE messages_fts SET content = new.content WHERE rowid = new.rowid;
END;

-- Assunto e resumo no mesmo tipo de índice, para ignorar acento e maiúscula.
CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
  id UNINDEXED,
  subject,
  overview,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
  INSERT INTO sessions_fts(rowid, id, subject, overview)
  VALUES (new.rowid, new.id, new.subject, new.overview);
END;

CREATE TRIGGER IF NOT EXISTS sessions_ad AFTER DELETE ON sessions BEGIN
  DELETE FROM sessions_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS sessions_au AFTER UPDATE OF subject, overview ON sessions BEGIN
  UPDATE sessions_fts SET subject = new.subject, overview = new.overview WHERE rowid = new.rowid;
END;
