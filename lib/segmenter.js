'use strict';

const SAMPLE_RATE = 16000;

/**
 * Divide o áudio contínuo em falas, como um detector de voz (VAD):
 * mede o volume (RMS) de cada trecho e fecha a fala depois de um tempo de silêncio.
 * Uma instância por fonte de áudio (microfone ou som do computador).
 */
class Segmenter {
  constructor({
    onUtterance,
    rmsThreshold = 0.012, // volume mínimo para contar como fala (0 a 1)
    silenceMs = 700, // silêncio que encerra uma fala
    minSpeechMs = 250, // falas mais curtas que isso são descartadas (ruído)
    maxUtteranceMs = 25000, // corte forçado para falas muito longas
    preRollMs = 300, // áudio guardado antes da fala, para não cortar o começo
  }) {
    Object.assign(this, { onUtterance, rmsThreshold, silenceMs, minSpeechMs, maxUtteranceMs, preRollMs });
    this.preRoll = [];
    this.preRollSamples = 0;
    this.reset();
  }

  reset() {
    this.chunks = [];
    this.samples = 0;
    this.speechSamples = 0;
    this.silenceSamples = 0;
    this.startedAt = null;
    this.speaking = false;
  }

  /** @param {Int16Array} pcm trecho de PCM 16 bits, 16 kHz mono. @param {number} nowMs */
  push(pcm, nowMs = Date.now()) {
    let soma = 0;
    for (let i = 0; i < pcm.length; i++) {
      const v = pcm[i] / 32768;
      soma += v * v;
    }
    const voz = Math.sqrt(soma / (pcm.length || 1)) > this.rmsThreshold;
    const ms = (n) => (n / SAMPLE_RATE) * 1000;

    if (!this.speaking) {
      if (voz) {
        this.speaking = true;
        this.startedAt = nowMs - ms(this.preRollSamples);
        this.chunks = [...this.preRoll];
        this.samples = this.preRollSamples;
        this.preRoll = [];
        this.preRollSamples = 0;
      } else {
        this.preRoll.push(pcm);
        this.preRollSamples += pcm.length;
        while (this.preRoll.length > 1 && ms(this.preRollSamples) > this.preRollMs) {
          this.preRollSamples -= this.preRoll.shift().length;
        }
        return;
      }
    }

    this.chunks.push(pcm);
    this.samples += pcm.length;
    if (voz) {
      this.speechSamples += pcm.length;
      this.silenceSamples = 0;
    } else {
      this.silenceSamples += pcm.length;
    }

    if (ms(this.silenceSamples) >= this.silenceMs || ms(this.samples) >= this.maxUtteranceMs) this.#fechar();
  }

  /** Encerra a fala em andamento (ao parar a reunião). */
  flush() {
    if (this.speaking) this.#fechar();
  }

  #fechar() {
    const falouMs = (this.speechSamples / SAMPLE_RATE) * 1000;
    const todo = new Int16Array(this.samples);
    let pos = 0;
    for (const c of this.chunks) {
      todo.set(c, pos);
      pos += c.length;
    }
    const inicio = this.startedAt;
    this.reset();
    if (falouMs >= this.minSpeechMs) this.onUtterance({ pcm: todo, startedAt: inicio });
  }
}

module.exports = { Segmenter, SAMPLE_RATE };
