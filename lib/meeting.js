'use strict';

const { randomUUID } = require('node:crypto');
const { Segmenter } = require('./segmenter');
const { formatarTranscricao } = require('./summary');

const FONTES = { mic: 'ME', sistema: 'THEY' };

/**
 * Uma reunião: recebe áudio das duas fontes, transcreve em tempo real,
 * e ao parar gera o resumo, grava e envia por e-mail.
 * Tudo que o usuário precisa ver sai por `emit(canal, dados)`.
 */
class Meeting {
  constructor({ store, userId, whisper, resumir, enviarEmail, config, emit, agora = Date.now }) {
    Object.assign(this, { store, userId, whisper, resumirFn: resumir, enviarEmail, config, emit, agora });
    this.id = null;
    this.segmentadores = {};
    this.emAndamento = new Set(); // transcrições ainda no ar
    this.ativa = false;
  }

  iniciar() {
    if (this.ativa) throw new Error('Já existe uma reunião em andamento.');
    this.id = randomUUID();
    this.inicio = this.agora();
    this.store.addSession({ id: this.id, userId: this.userId, status: 'active', startedAt: Math.floor(this.inicio / 1000) });
    for (const [fonte, falante] of Object.entries(FONTES)) {
      this.segmentadores[fonte] = new Segmenter({ onUtterance: (u) => this.#transcrever(falante, u) });
    }
    this.ativa = true;
    this.emit('reuniao:estado', { fase: 'gravando', id: this.id });
  }

  /** @param {'mic'|'sistema'} fonte @param {Int16Array} pcm */
  receberAudio(fonte, pcm) {
    if (this.ativa) this.segmentadores[fonte]?.push(pcm, this.agora());
  }

  #transcrever(speaker, { pcm, startedAt }) {
    const tarefa = this.whisper
      .transcrever(pcm)
      .then((text) => {
        if (!text) return;
        const fala = { speaker, text, timestamp: startedAt };
        this.store.addMessage({ id: randomUUID(), sessionId: this.id, content: text, speaker, timestamp: startedAt });
        this.emit('reuniao:fala', fala);
      })
      .catch((erro) => this.emit('reuniao:aviso', `Falha ao transcrever um trecho: ${erro.message}`))
      .finally(() => this.emAndamento.delete(tarefa));
    this.emAndamento.add(tarefa);
  }

  async parar() {
    if (!this.ativa) throw new Error('Nenhuma reunião em andamento.');
    this.ativa = false;
    this.emit('reuniao:estado', { fase: 'finalizando' });
    Object.values(this.segmentadores).forEach((s) => s.flush());
    await Promise.all([...this.emAndamento]);

    const id = this.id;
    const falas = this.store.getTranscript(id);
    const transcricao = formatarTranscricao(falas);
    const fim = this.agora();
    const cfg = this.config();
    const data = new Date(this.inicio).toLocaleDateString('pt-BR').replaceAll('/', '-');

    if (!falas.length) {
      this.store.endSession(id, { endedAt: Math.floor(fim / 1000), subject: 'Reunião sem fala captada', overview: null, pending: [] });
      this.emit('reuniao:final', { id, vazia: true });
      return;
    }

    let resumo = null;
    let erroResumo = null;
    this.emit('reuniao:estado', { fase: 'resumindo' });
    try {
      resumo = await this.resumirFn(transcricao, { apiKey: cfg.openaiKey, model: cfg.openaiModel });
    } catch (e) {
      erroResumo = e.message;
    }

    const titulo = resumo?.titulo || `Reunião de ${data}`;
    this.store.endSession(id, {
      endedAt: Math.floor(fim / 1000),
      subject: titulo,
      overview: resumo?.resumo ?? null,
      pending: resumo?.pendencias ?? [],
    });
    this.emit('reuniao:final', { id, titulo, resumo, erroResumo, transcricao });

    if (cfg.enviarAutomatico) await this.enviarPorEmail(id, { resumo, erroResumo });
  }

  /** Envia (ou reenvia) a transcrição de uma reunião já encerrada. */
  async enviarPorEmail(id, { resumo: resumoCompleto = null, erroResumo = null } = {}) {
    const cfg = this.config();
    const sessao = this.store.getSession(id);
    const transcricao = formatarTranscricao(this.store.getTranscript(id));
    // Sem o resumo completo em mãos (reenvio), usa o que foi gravado no banco.
    const resumo =
      resumoCompleto ??
      (sessao.overview ? { resumo: sessao.overview, decisoes: [], pendencias: sessao.pending, duvidas: [] } : null);
    this.emit('reuniao:email', { estado: 'enviando' });
    try {
      await this.enviarEmail({
        usuario: cfg.smtpUser,
        senha: cfg.smtpPass,
        destinatario: cfg.destinatario,
        titulo: sessao.subject,
        data: new Date(sessao.started_at * 1000).toLocaleDateString('pt-BR').replaceAll('/', '-'),
        resumo,
        erroResumo,
        transcricao,
      });
      this.emit('reuniao:email', { estado: 'enviado', para: cfg.destinatario });
    } catch (e) {
      this.emit('reuniao:email', { estado: 'erro', mensagem: e.message });
    }
  }
}

module.exports = { Meeting, FONTES };
