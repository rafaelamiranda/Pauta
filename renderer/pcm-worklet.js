// Roda na thread de áudio: junta blocos de 128 amostras em trechos de ~100 ms (16 kHz).
class Coletor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buf = new Float32Array(1600);
    this.n = 0;
  }

  process(entradas) {
    const canal = entradas[0]?.[0];
    if (!canal) return true;
    for (let i = 0; i < canal.length; i++) {
      this.buf[this.n++] = canal[i];
      if (this.n === this.buf.length) {
        const pcm = new Int16Array(this.n);
        for (let j = 0; j < this.n; j++) pcm[j] = Math.max(-1, Math.min(1, this.buf[j])) * 32767;
        this.port.postMessage(pcm.buffer, [pcm.buffer]);
        this.n = 0;
      }
    }
    return true;
  }
}

registerProcessor('coletor-pcm', Coletor);
