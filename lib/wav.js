'use strict';

/** Empacota PCM 16 bits mono em um arquivo WAV. */
function pcm16ToWav(pcm, sampleRate = 16000) {
  const dados = Buffer.from(pcm.buffer, pcm.byteOffset, pcm.byteLength);
  const cab = Buffer.alloc(44);
  cab.write('RIFF', 0);
  cab.writeUInt32LE(36 + dados.length, 4);
  cab.write('WAVEfmt ', 8);
  cab.writeUInt32LE(16, 16); // tamanho do bloco fmt
  cab.writeUInt16LE(1, 20); // PCM
  cab.writeUInt16LE(1, 22); // mono
  cab.writeUInt32LE(sampleRate, 24);
  cab.writeUInt32LE(sampleRate * 2, 28);
  cab.writeUInt16LE(2, 32);
  cab.writeUInt16LE(16, 34);
  cab.write('data', 36);
  cab.writeUInt32LE(dados.length, 40);
  return Buffer.concat([cab, dados]);
}

module.exports = { pcm16ToWav };
