'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { DatabaseSync } = require('node:sqlite');

/**
 * Transforma o texto digitado em uma consulta FTS5 segura.
 * Cada palavra vira um termo entre aspas com prefixo ("reun"*), todos com AND.
 * Evita erro de sintaxe com aspas, parênteses, hífens etc.
 */
function toFtsQuery(text) {
  const terms = String(text)
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
  if (terms.length === 0) return null;
  return terms.map((t) => `"${t}"*`).join(' ');
}

class ConversationStore {
  constructor(dbPath = ':memory:') {
    this.db = new DatabaseSync(dbPath);
    this.db.exec('PRAGMA foreign_keys = ON');
    this.db.exec(fs.readFileSync(path.join(__dirname, 'schema.sql'), 'utf8'));
    this.#backfillIndex();
  }

  /** Reconstrói os índices quando há dados mas o índice está vazio. */
  #backfillIndex() {
    const sess = this.db.prepare('SELECT COUNT(*) AS n FROM sessions').get().n;
    const sessFts = this.db.prepare('SELECT COUNT(*) AS n FROM sessions_fts').get().n;
    if (sess > 0 && sessFts === 0) {
      this.db.exec(`
        INSERT INTO sessions_fts(rowid, id, subject, overview)
        SELECT rowid, id, subject, overview FROM sessions`);
    }
    const msgs = this.db.prepare('SELECT COUNT(*) AS n FROM messages').get().n;
    const fts = this.db.prepare('SELECT COUNT(*) AS n FROM messages_fts').get().n;
    if (msgs > 0 && fts === 0) {
      this.db.exec(`
        INSERT INTO messages_fts(rowid, id, session_id, content)
        SELECT rowid, id, session_id, content FROM messages`);
    }
  }

  addSession({ id, userId, subject = null, overview = null, pending = null, status = 'ended', startedAt, endedAt = null }) {
    this.db
      .prepare(
        `INSERT INTO sessions (id, user_id, subject, overview, pending, status, started_at, ended_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)`
      )
      .run(id, userId, subject, overview, pending && JSON.stringify(pending), status, startedAt, endedAt);
  }

  /** Encerra a reunião e grava o resumo e as pendências (lista de objetos). */
  endSession(id, { endedAt, subject = null, overview = null, pending = null }) {
    this.db
      .prepare(
        `UPDATE sessions
            SET status = 'ended', ended_at = ?, subject = COALESCE(?, subject),
                overview = ?, pending = ?
          WHERE id = ?`
      )
      .run(endedAt, subject, overview, pending && JSON.stringify(pending), id);
  }

  getSession(id) {
    const row = this.db.prepare('SELECT * FROM sessions WHERE id = ?').get(id);
    if (!row) return null;
    return { ...row, pending: row.pending ? JSON.parse(row.pending) : [] };
  }

  /** Falas da reunião em ordem de tempo. */
  getTranscript(sessionId) {
    return this.db
      .prepare('SELECT speaker, content AS text, timestamp FROM messages WHERE session_id = ? ORDER BY timestamp, rowid')
      .all(sessionId)
      .map((r) => ({ ...r }));
  }

  addMessage({ id, sessionId, role = 'user', content, speaker = null, timestamp }) {
    this.db
      .prepare('INSERT INTO messages (id, session_id, role, content, speaker, timestamp) VALUES (?, ?, ?, ?, ?, ?)')
      .run(id, sessionId, role, content, speaker, timestamp);
  }

  /**
   * Busca conversas do usuário. Uma conversa entra se o termo aparece
   * no assunto, no resumo ou em qualquer mensagem.
   * Sem termo, devolve todas as conversas (paginadas).
   *
   * @returns {{ sessions: Array, total: number }}
   */
  searchSessions(userId, { search = null, limit = 20, offset = 0, excludeActive = true } = {}) {
    if (!userId) return { sessions: [], total: 0 };

    const pageSize = Math.max(1, Math.min(200, Number(limit) || 20));
    const skip = Math.max(0, Number(offset) || 0);

    const conditions = ['s.user_id = ?'];
    const params = [userId];

    if (excludeActive) conditions.push("s.status != 'active'");

    const text = search && search.trim();
    const ftsQuery = text ? toFtsQuery(text) : null;
    if (text) {
      if (!ftsQuery) return { sessions: [], total: 0 }; // só símbolos, nada a buscar
      conditions.push(`(
        s.id IN (SELECT id FROM sessions_fts WHERE sessions_fts MATCH ?)
        OR s.id IN (SELECT session_id FROM messages_fts WHERE messages_fts MATCH ?)
      )`);
      params.push(ftsQuery, ftsQuery);
    }

    const where = `WHERE ${conditions.join(' AND ')}`;
    const sessions = this.db
      .prepare(
        `SELECT s.id, s.subject, s.overview, s.status, s.started_at, s.ended_at
           FROM sessions s ${where}
          ORDER BY (s.ended_at IS NULL) DESC, COALESCE(s.ended_at, s.started_at) DESC
          LIMIT ? OFFSET ?`
      )
      .all(...params, pageSize, skip)
      .map((row) => ({ ...row }));

    const { n: total } = this.db.prepare(`SELECT COUNT(*) AS n FROM sessions s ${where}`).get(...params);

    if (ftsQuery) {
      const snip = this.db.prepare(
        `SELECT snippet(messages_fts, 2, '[', ']', '…', 12) AS trecho
           FROM messages_fts
          WHERE session_id = ? AND messages_fts MATCH ?
          ORDER BY rank LIMIT 1`
      );
      for (const s of sessions) s.snippet = snip.get(s.id, ftsQuery)?.trecho ?? null;
    }

    return { sessions, total };
  }

  close() {
    this.db.close();
  }
}

module.exports = { ConversationStore, toFtsQuery };
