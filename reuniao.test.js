'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { ConversationStore } = require('./search');
const { Segmenter } = require('./lib/segmenter');
const { pcm16ToWav } = require('./lib/wav');
const { WhisperClient, limpar } = require('./lib/whisper-client');
const { resumir, formatarTranscricao } = require('./lib/summary');
const { montarEmail, enviar } = require('./lib/mailer');
const { Settings } = require('./lib/settings');
const { Meeting } = require('./lib/meeting');

const bloco = (valor, n = 1600) => new Int16Array(n).fill(valor);
const FALA = 6000;
const SILENCIO = 0;

test('segmenter fecha a fala após o silêncio e descarta ruído curto', () => {
  const falas = [];
  const seg = new Segmenter({ onUtterance: (u) => falas.push(u) });
  for (let i = 0; i < 5; i++) seg.push(bloco(SILENCIO)); // silêncio inicial
  for (let i = 0; i < 6; i++) seg.push(bloco(FALA)); // 600 ms de fala
  assert.equal(falas.length, 0, 'ainda não passou o silêncio');
  for (let i = 0; i < 8; i++) seg.push(bloco(SILENCIO)); // 800 ms
  assert.equal(falas.length, 1);
  assert.ok(falas[0].pcm.length >= 6 * 1600);

  seg.push(bloco(FALA)); // 100 ms: curto demais
  for (let i = 0; i < 8; i++) seg.push(bloco(SILENCIO));
  assert.equal(falas.length, 1, 'ruído curto descartado');
});

test('segmenter: flush fecha a fala em andamento e corta falas longas', () => {
  const falas = [];
  const seg = new Segmenter({ onUtterance: (u) => falas.push(u), maxUtteranceMs: 2000 });
  for (let i = 0; i < 5; i++) seg.push(bloco(FALA));
  seg.flush();
  assert.equal(falas.length, 1);
  for (let i = 0; i < 25; i++) seg.push(bloco(FALA)); // 2,5 s contínuos
  assert.equal(falas.length, 2, 'corte forçado em 2 s');
});

test('wav tem cabeçalho correto', () => {
  const wav = pcm16ToWav(new Int16Array(160));
  assert.equal(wav.toString('ascii', 0, 4), 'RIFF');
  assert.equal(wav.toString('ascii', 8, 12), 'WAVE');
  assert.equal(wav.readUInt32LE(24), 16000);
  assert.equal(wav.readUInt32LE(40), 320);
  assert.equal(wav.length, 44 + 320);
});

test('whisper-client envia o áudio e filtra ruído', async () => {
  let pedido;
  const fake = async (url, init) => {
    pedido = { url, campos: Object.fromEntries(init.body) };
    return { ok: true, json: async () => ({ text: ' Olá  mundo ' }) };
  };
  const c = new WhisperClient({ baseUrl: 'http://x', fetchImpl: fake });
  assert.equal(await c.transcrever(new Int16Array(160)), 'Olá mundo');
  assert.equal(pedido.url, 'http://x/inference');
  assert.equal(pedido.campos.language, 'pt');
  assert.equal(limpar('[BLANK_AUDIO]'), '');
  assert.equal(limpar('(música)'), '');
  assert.equal(limpar(' ... '), '');
});

test('whisper-client: erro de um pedido não trava a fila', async () => {
  let n = 0;
  const fake = async () => (++n === 1 ? { ok: false, status: 500 } : { ok: true, json: async () => ({ text: 'ok' }) });
  const c = new WhisperClient({ baseUrl: 'http://x', fetchImpl: fake });
  const [a, b] = await Promise.allSettled([c.transcrever(new Int16Array(8)), c.transcrever(new Int16Array(8))]);
  assert.equal(a.status, 'rejected');
  assert.equal(b.value, 'ok');
});

test('formatarTranscricao marca tempo e quem falou', () => {
  const t = formatarTranscricao([
    { speaker: 'ME', text: 'Bom dia', timestamp: 1000 },
    { speaker: 'THEY', text: 'Bom dia, tudo bem?', timestamp: 66000 },
  ]);
  assert.equal(t, '[00:00:00] Eu: Bom dia\n[00:01:05] Outros: Bom dia, tudo bem?');
});

test('resumir chama a OpenAI com esquema e devolve o JSON', async () => {
  const resultado = { titulo: 'T', resumo: 'R', decisoes: [], pendencias: [{ tarefa: 'a', responsavel: null, prazo: null }], duvidas: [] };
  let corpo;
  const fake = async (url, init) => {
    corpo = { url, auth: init.headers.Authorization, ...JSON.parse(init.body) };
    return { ok: true, json: async () => ({ choices: [{ message: { content: JSON.stringify(resultado) } }] }) };
  };
  const r = await resumir('[00:00:00] Eu: oi', { apiKey: 'sk-teste', model: 'm', fetchImpl: fake });
  assert.deepEqual(r, resultado);
  assert.equal(corpo.url, 'https://api.openai.com/v1/chat/completions');
  assert.equal(corpo.auth, 'Bearer sk-teste');
  assert.equal(corpo.response_format.type, 'json_schema');
  await assert.rejects(resumir('x', { apiKey: '' }), /Chave/);
  await assert.rejects(
    resumir('x', { apiKey: 'k', fetchImpl: async () => ({ ok: false, status: 401, json: async () => ({ error: { message: 'chave inválida' } }) }) }),
    /401.*chave inválida/
  );
});

test('montarEmail escapa HTML e anexa a transcrição; enviar usa o SMTP do Gmail', async () => {
  const email = montarEmail({
    titulo: 'Vendas <b>', data: '13-10-2026', transcricao: 'linha',
    resumo: { resumo: 'ok', decisoes: ['d'], pendencias: [{ tarefa: 'enviar', responsavel: 'Ana', prazo: 'sexta' }], duvidas: [] },
  });
  assert.match(email.html, /enviar \(responsável: Ana\) \(prazo: sexta\)/);
  assert.doesNotMatch(email.html, /<script/);
  assert.equal(email.attachments[0].content, 'linha');
  assert.match(montarEmail({ titulo: 'x', data: 'd', resumo: null, erroResumo: 'sem chave', transcricao: '' }).text, /Não foi possível gerar o resumo: sem chave/);

  let cfg, enviado;
  await enviar({
    usuario: 'eu@gmail.com', senha: 'abcd', destinatario: 'para@x.com', email,
    criarTransporte: (c) => { cfg = c; return { sendMail: async (m) => { enviado = m; } }; },
  });
  assert.equal(cfg.host, 'smtp.gmail.com');
  assert.equal(enviado.to, 'para@x.com');
  await assert.rejects(enviar({ usuario: '', senha: '', destinatario: '', email }), /não configurado/);
});

test('settings criptografa segredos e não os devolve para a tela', () => {
  const arquivo = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'cfg-')), 'config.json');
  const cifra = {
    criptografar: (t) => Buffer.from(t).reverse().toString('base64'),
    descriptografar: (t) => Buffer.from(t, 'base64').reverse().toString(),
  };
  const s = new Settings(arquivo, cifra);
  s.salvar({ openaiKey: 'sk-1', smtpUser: 'eu@gmail.com', smtpPass: 'senha' });
  assert.doesNotMatch(fs.readFileSync(arquivo, 'utf8'), /sk-1|senha/);
  assert.equal(s.obter().openaiKey, 'sk-1');
  const tela = s.paraTela();
  assert.equal(tela.openaiKey, '');
  assert.equal(tela.openaiKeyDefinida, true);
  s.salvar({ openaiKey: '', destinatario: 'a@b.com' }); // vazio mantém a chave
  assert.equal(s.obter().openaiKey, 'sk-1');
  assert.equal(s.obter().destinatario, 'a@b.com');
});

test('reunião de ponta a ponta: transcreve, resume, grava e envia', async () => {
  const store = new ConversationStore();
  const eventos = [];
  const textos = ['Vamos fechar o contrato', 'Eu envio a proposta até sexta'];
  const whisper = { transcrever: async () => textos.shift() };
  let emailEnviado;
  const resumo = { titulo: 'Contrato', resumo: 'Fechar contrato', decisoes: [], pendencias: [{ tarefa: 'Enviar proposta', responsavel: 'Eu', prazo: 'sexta' }], duvidas: [] };
  let t = 1_000_000;
  const m = new Meeting({
    store, userId: 'u', whisper,
    resumir: async () => resumo,
    enviarEmail: async (d) => { emailEnviado = d; },
    config: () => ({ openaiKey: 'k', enviarAutomatico: true, smtpUser: 'a', smtpPass: 'b', destinatario: 'c' }),
    emit: (canal, dados) => eventos.push([canal, dados]),
    agora: () => (t += 1000),
  });
  m.iniciar();
  assert.throws(() => m.iniciar(), /andamento/);
  for (const fonte of ['mic', 'sistema']) {
    for (let i = 0; i < 6; i++) m.receberAudio(fonte, bloco(FALA));
  }
  await m.parar();

  const id = m.id;
  const sessao = store.getSession(id);
  assert.equal(sessao.status, 'ended');
  assert.equal(sessao.subject, 'Contrato');
  assert.equal(sessao.pending[0].tarefa, 'Enviar proposta');
  assert.equal(store.getTranscript(id).length, 2);
  assert.equal(store.searchSessions('u', { search: 'proposta' }).total, 1, 'transcrição fica pesquisável');
  assert.equal(emailEnviado.destinatario, 'c');
  assert.match(emailEnviado.transcricao, /Eu: .*\n\[.*\] Outros: /s);
  const canais = eventos.map((e) => e[0]);
  assert.ok(canais.includes('reuniao:fala') && canais.includes('reuniao:final'));
  assert.deepEqual(eventos.findLast((e) => e[0] === 'reuniao:email')[1].estado, 'enviado');
});

test('reunião: falha no resumo ainda grava e envia a transcrição', async () => {
  const store = new ConversationStore();
  const eventos = [];
  let email;
  const m = new Meeting({
    store, userId: 'u', whisper: { transcrever: async () => 'só uma fala' },
    resumir: async () => { throw new Error('sem rede'); },
    enviarEmail: async (d) => { email = d; },
    config: () => ({ enviarAutomatico: true }),
    emit: (c, d) => eventos.push([c, d]),
  });
  m.iniciar();
  for (let i = 0; i < 6; i++) m.receberAudio('mic', bloco(FALA));
  await m.parar();
  const final = eventos.find((e) => e[0] === 'reuniao:final')[1];
  assert.equal(final.erroResumo, 'sem rede');
  assert.equal(email.erroResumo, 'sem rede');
  assert.equal(store.getTranscript(m.id).length, 1);
});

test('reunião sem fala não chama resumo nem e-mail', async () => {
  const store = new ConversationStore();
  let chamou = false;
  const eventos = [];
  const m = new Meeting({
    store, userId: 'u', whisper: { transcrever: async () => '' },
    resumir: async () => { chamou = true; }, enviarEmail: async () => { chamou = true; },
    config: () => ({ enviarAutomatico: true }), emit: (c, d) => eventos.push([c, d]),
  });
  m.iniciar();
  await m.parar();
  assert.equal(chamou, false);
  assert.equal(eventos.find((e) => e[0] === 'reuniao:final')[1].vazia, true);
});
