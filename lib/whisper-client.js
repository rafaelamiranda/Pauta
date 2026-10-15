'use strict';

const { pcm16ToWav } = require('./wav');

// O whisper costuma "inventar" estas marcações quando ouve só ruído ou silêncio.
const RUIDO = /^[\s.\-–—]*$|^\s*[\[(*].*[\])*]\s*$/;

function limpar(texto) {
  const t = String(texto || '').replace(/\s+/g, ' ').trim();
  return RUIDO.test(t) ? '' : t;
}

/** Cliente do whisper-server (whisper.cpp). Atende um pedido por vez, em fila. */
class WhisperClient {
  constructor({ baseUrl, language = 'pt', fetchImpl = fetch }) {
    this.baseUrl = baseUrl;
    this.language = language;
    this.fetch = fetchImpl;
    this.fila = Promise.resolve();
  }

  transcrever(pcm) {
    const tarefa = this.fila.then(() => this.#enviar(pcm));
    this.fila = tarefa.catch(() => {}); // um erro não trava os próximos pedidos
    return tarefa;
  }

  async #enviar(pcm) {
    const form = new FormData();
    form.append('file', new Blob([pcm16ToWav(pcm)], { type: 'audio/wav' }), 'fala.wav');
    form.append('response_format', 'json');
    form.append('temperature', '0.0');
    form.append('language', this.language);
    const res = await this.fetch(`${this.baseUrl}/inference`, { method: 'POST', body: form });
    if (!res.ok) throw new Error(`whisper-server respondeu ${res.status}`);
    const json = await res.json();
    return limpar(json.text);
  }
}

module.exports = { WhisperClient, limpar };
