'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');

const PORTA = 8178;

/** Sobe e derruba o whisper-server (whisper.cpp) instalado por `npm run setup`. */
class WhisperServer {
  constructor({ pasta, modelo = 'ggml-small.bin', language = 'pt', porta = PORTA }) {
    this.exe = path.join(pasta, 'bin', 'whisper-server.exe');
    this.modelo = path.join(pasta, 'models', modelo);
    this.language = language;
    this.porta = porta;
    this.proc = null;
  }

  get url() {
    return `http://127.0.0.1:${this.porta}`;
  }

  instalado() {
    return fs.existsSync(this.exe) && fs.existsSync(this.modelo);
  }

  async iniciar(timeoutMs = 60000) {
    if (this.proc) return;
    if (!this.instalado()) throw new Error('Whisper não instalado. Rode "npm run setup" na pasta do app.');
    this.proc = spawn(
      this.exe,
      ['-m', this.modelo, '--host', '127.0.0.1', '--port', String(this.porta), '-l', this.language],
      { cwd: path.dirname(this.exe), windowsHide: true, stdio: 'ignore' }
    );
    this.proc.once('exit', () => { this.proc = null; });

    const limite = Date.now() + timeoutMs;
    while (Date.now() < limite) {
      if (!this.proc) throw new Error('O whisper-server fechou ao iniciar.');
      try {
        await fetch(this.url); // qualquer resposta HTTP indica que já está de pé
        return;
      } catch {
        await new Promise((r) => setTimeout(r, 400));
      }
    }
    this.parar();
    throw new Error('O whisper-server demorou demais para iniciar.');
  }

  parar() {
    this.proc?.kill();
    this.proc = null;
  }
}

module.exports = { WhisperServer };
