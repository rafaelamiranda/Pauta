'use strict';

const fs = require('node:fs');

const SEGREDOS = ['openaiKey', 'smtpPass'];
const PADRAO = {
  openaiKey: '',
  openaiModel: 'gpt-4o-mini',
  smtpUser: '', // endereço do Gmail que envia
  smtpPass: '', // senha de app do Gmail
  destinatario: '',
  enviarAutomatico: true,
};

/**
 * Configurações em arquivo JSON. Chaves e senhas ficam criptografadas.
 * `cifra` = { criptografar(texto) -> texto, descriptografar(texto) -> texto }
 */
class Settings {
  constructor(arquivo, cifra) {
    this.arquivo = arquivo;
    this.cifra = cifra;
  }

  #ler() {
    try {
      return JSON.parse(fs.readFileSync(this.arquivo, 'utf8'));
    } catch {
      return {};
    }
  }

  /** Valores completos, com segredos descriptografados (uso interno do app). */
  obter() {
    const bruto = this.#ler();
    const out = { ...PADRAO };
    for (const k of Object.keys(PADRAO)) {
      if (bruto[k] === undefined) continue;
      out[k] = SEGREDOS.includes(k) && bruto[k] ? this.cifra.descriptografar(bruto[k]) : bruto[k];
    }
    return out;
  }

  /** Versão para a tela: nunca devolve o segredo, só se ele existe. */
  paraTela() {
    const v = this.obter();
    for (const k of SEGREDOS) {
      v[`${k}Definida`] = Boolean(v[k]);
      v[k] = '';
    }
    return v;
  }

  /** Salva campos. Segredo vazio mantém o valor anterior. */
  salvar(novos) {
    const atual = this.#ler();
    for (const k of Object.keys(PADRAO)) {
      if (novos[k] === undefined) continue;
      if (SEGREDOS.includes(k)) {
        if (novos[k] !== '') atual[k] = this.cifra.criptografar(String(novos[k]));
      } else {
        atual[k] = novos[k];
      }
    }
    fs.writeFileSync(this.arquivo, JSON.stringify(atual, null, 2));
  }
}

module.exports = { Settings, PADRAO };
