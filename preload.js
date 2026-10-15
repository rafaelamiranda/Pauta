'use strict';

const { contextBridge, ipcRenderer } = require('electron');

const ouvir = (canal) => (fn) => ipcRenderer.on(canal, (_e, dados) => fn(dados));

contextBridge.exposeInMainWorld('busca', {
  buscar: (q, limit, offset) => ipcRenderer.invoke('buscar', { q, limit, offset }),
  conversa: (id) => ipcRenderer.invoke('conversa', id),
  esconder: () => ipcRenderer.send('esconder'),
  aoFocar: ouvir('foco-busca'),

  lerConfig: () => ipcRenderer.invoke('config:ler'),
  salvarConfig: (c) => ipcRenderer.invoke('config:salvar', c),

  iniciarReuniao: () => ipcRenderer.invoke('reuniao:iniciar'),
  pararReuniao: () => ipcRenderer.invoke('reuniao:parar'),
  reenviarEmail: (id) => ipcRenderer.invoke('reuniao:reenviar', id),
  enviarAudio: (fonte, buffer) => ipcRenderer.send('reuniao:audio', { fonte, buffer }),
  aoEstado: ouvir('reuniao:estado'),
  aoFalar: ouvir('reuniao:fala'),
  aoAviso: ouvir('reuniao:aviso'),
  aoFinal: ouvir('reuniao:final'),
  aoEmail: ouvir('reuniao:email'),
});
