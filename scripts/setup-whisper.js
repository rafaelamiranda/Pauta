'use strict';

// Baixa o whisper.cpp (Windows, CPU) e o modelo de voz para ./whisper.
// Uso: npm run setup   (variável WHISPER_MODEL muda o modelo; padrão: small)

const fs = require('node:fs');
const path = require('node:path');
const { Readable } = require('node:stream');
const { pipeline } = require('node:stream/promises');
const { execFileSync } = require('node:child_process');

const RAIZ = path.join(__dirname, '..', 'whisper');
const BIN = path.join(RAIZ, 'bin');
const MODELOS = path.join(RAIZ, 'models');
const MODELO = process.env.WHISPER_MODEL || 'small';

async function baixar(url, destino) {
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) throw new Error(`Falha ao baixar ${url}: ${res.status}`);
  const total = Number(res.headers.get('content-length')) || 0;
  let lido = 0;
  let ultimo = 0;
  const fonte = Readable.fromWeb(res.body);
  fonte.on('data', (c) => {
    lido += c.length;
    if (total && Date.now() - ultimo > 1000) {
      ultimo = Date.now();
      process.stdout.write(`\r  ${(lido / 1e6).toFixed(0)} de ${(total / 1e6).toFixed(0)} MB`);
    }
  });
  await pipeline(fonte, fs.createWriteStream(destino));
  process.stdout.write('\n');
}

async function urlDoPrograma() {
  const res = await fetch('https://api.github.com/repos/ggml-org/whisper.cpp/releases?per_page=30');
  if (!res.ok) throw new Error(`GitHub respondeu ${res.status}`);
  for (const rel of await res.json()) {
    const arq = rel.assets.find((a) => a.name === 'whisper-bin-x64.zip');
    if (arq) return arq.browser_download_url;
  }
  throw new Error('Não achei whisper-bin-x64.zip nas versões recentes do whisper.cpp.');
}

async function main() {
  fs.mkdirSync(BIN, { recursive: true });
  fs.mkdirSync(MODELOS, { recursive: true });

  if (!fs.existsSync(path.join(BIN, 'whisper-server.exe'))) {
    console.log('Baixando o programa whisper.cpp...');
    const zip = path.join(RAIZ, 'whisper-bin.zip');
    await baixar(await urlDoPrograma(), zip);
    execFileSync('tar', ['-xf', zip, '-C', RAIZ]); // o tar do Windows 10+ lê .zip
    fs.cpSync(path.join(RAIZ, 'Release'), BIN, { recursive: true });
    fs.rmSync(path.join(RAIZ, 'Release'), { recursive: true, force: true });
    fs.rmSync(zip);
  } else {
    console.log('Programa whisper.cpp já instalado.');
  }

  const modelo = path.join(MODELOS, `ggml-${MODELO}.bin`);
  if (!fs.existsSync(modelo)) {
    console.log(`Baixando o modelo "${MODELO}" (pode passar de 400 MB)...`);
    const parcial = `${modelo}.parte`;
    await baixar(`https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-${MODELO}.bin`, parcial);
    fs.renameSync(parcial, modelo);
  } else {
    console.log(`Modelo "${MODELO}" já instalado.`);
  }
  console.log('Pronto. Rode: npm start');
}

main().catch((e) => {
  console.error(`\nErro: ${e.message}`);
  process.exit(1);
});
