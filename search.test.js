'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { ConversationStore, toFtsQuery } = require('./search');

function seed() {
  const s = new ConversationStore();
  s.addSession({ id: 's1', userId: 'u1', subject: 'Reunião de vendas', overview: 'Metas do trimestre', startedAt: 100, endedAt: 200 });
  s.addSession({ id: 's2', userId: 'u1', subject: 'Orçamento', overview: 'Custos', startedAt: 300, endedAt: 400 });
  s.addSession({ id: 's3', userId: 'u1', subject: 'Ativa', startedAt: 500, status: 'active' });
  s.addSession({ id: 's4', userId: 'u2', subject: 'Reunião de outro usuário', startedAt: 100, endedAt: 200 });
  s.addMessage({ id: 'm1', sessionId: 's2', role: 'user', content: 'Precisamos revisar o contrato com o fornecedor', timestamp: 310 });
  s.addMessage({ id: 'm2', sessionId: 's3', role: 'user', content: 'contrato ativo', timestamp: 510 });
  return s;
}

test('acha pelo assunto, sem diferenciar acento nem maiúscula', () => {
  const { sessions } = seed().searchSessions('u1', { search: 'REUNIAO' });
  assert.deepEqual(sessions.map((s) => s.id), ['s1']);
});

test('acha pelo conteúdo da mensagem e devolve trecho', () => {
  const { sessions, total } = seed().searchSessions('u1', { search: 'contrato' });
  assert.equal(total, 1);
  assert.equal(sessions[0].id, 's2');
  assert.match(sessions[0].snippet, /\[contrato\]/);
});

test('aceita prefixo da palavra', () => {
  const { sessions } = seed().searchSessions('u1', { search: 'fornec' });
  assert.deepEqual(sessions.map((s) => s.id), ['s2']);
});

test('não vaza conversa de outro usuário e oculta sessão ativa', () => {
  const store = seed();
  assert.equal(store.searchSessions('u1', { search: 'outro' }).total, 0);
  assert.equal(store.searchSessions('u1', { search: 'contrato' }).sessions.length, 1);
  assert.equal(store.searchSessions('u1', { search: 'contrato', excludeActive: false }).total, 2);
});

test('texto com símbolos não quebra a consulta', () => {
  const store = seed();
  for (const q of ['"', 'a OR (', '-', 'NEAR(', "o'brien", '%', '_']) {
    assert.doesNotThrow(() => store.searchSessions('u1', { search: q }));
  }
});

test('mensagem editada ou apagada atualiza o índice', () => {
  const store = seed();
  store.db.prepare("UPDATE messages SET content = 'nada relevante' WHERE id = 'm1'").run();
  assert.equal(store.searchSessions('u1', { search: 'contrato' }).total, 0);
  store.db.prepare("UPDATE messages SET content = 'prazo final' WHERE id = 'm1'").run();
  assert.equal(store.searchSessions('u1', { search: 'prazo' }).total, 1);
  store.db.prepare("DELETE FROM messages WHERE id = 'm1'").run();
  assert.equal(store.searchSessions('u1', { search: 'prazo' }).total, 0);
});

test('sem termo lista tudo, paginado', () => {
  const store = seed();
  const page = store.searchSessions('u1', { limit: 1, offset: 1 });
  assert.equal(page.total, 2);
  assert.equal(page.sessions.length, 1);
});

test('toFtsQuery', () => {
  assert.equal(toFtsQuery('reunião vendas'), '"reunião"* "vendas"*');
  assert.equal(toFtsQuery('!!!'), null);
});
