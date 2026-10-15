'use strict';

const path = require('node:path');
const {
  app, BrowserWindow, desktopCapturer, globalShortcut, ipcMain, safeStorage, screen, session,
} = require('electron');
const { ConversationStore } = require('./search');
const { seedIfEmpty } = require('./seed');
const { Settings } = require('./lib/settings');
const { WhisperServer } = require('./lib/whisper-server');
const { WhisperClient } = require('./lib/whisper-client');
const { Meeting } = require('./lib/meeting');
const { resumir } = require('./lib/summary');
const { montarEmail, enviar } = require('./lib/mailer');

const USER_ID = 'local';
const ATALHO = 'CommandOrControl+Shift+Space';
const LARGURA = 760;
const ALTURA = 620;

let store;
let win;
let settings;
let whisperServer;
let reuniao = null;

const cifra = {
  criptografar(texto) {
    if (!safeStorage.isEncryptionAvailable()) throw new Error('Criptografia do Windows indisponível; não é seguro guardar senhas.');
    return safeStorage.encryptString(texto).toString('base64');
  },
  descriptografar: (b64) => safeStorage.decryptString(Buffer.from(b64, 'base64')),
};

function criarJanela() {
  const { x, y, width } = screen.getPrimaryDisplay().workArea;
  win = new BrowserWindow({
    width: LARGURA,
    height: ALTURA,
    x: Math.round(x + (width - LARGURA) / 2),
    y: y + 60,
    transparent: true, // fundo transparente: só o painel aparece sobre a tela
    frame: false,
    resizable: false,
    hasShadow: false,
    skipTaskbar: true,
    alwaysOnTop: true,
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  win.setAlwaysOnTop(true, 'pop-up-menu'); // mesmo nível usado por overlays no Windows
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  win.on('closed', () => { win = null; });
}

const enviarParaTela = (canal, dados) => win?.webContents.send(canal, dados);

function mostrar() {
  if (!win) return;
  win.show();
  win.focus();
  enviarParaTela('foco-busca');
}

function alternar() {
  if (win && win.isVisible()) win.hide();
  else mostrar();
}

/** Libera microfone e a captura do som do computador (loopback) para a janela. */
function configurarCaptura() {
  const ses = session.defaultSession;
  ses.setPermissionRequestHandler((_wc, permissao, ok) => ok(permissao === 'media'));
  ses.setDisplayMediaRequestHandler(async (_req, responder) => {
    const [tela] = await desktopCapturer.getSources({ types: ['screen'] });
    responder({ video: tela, audio: 'loopback' });
  });
}

function novaReuniao() {
  const cliente = new WhisperClient({ baseUrl: whisperServer.url });
  return new Meeting({
    store,
    userId: USER_ID,
    whisper: cliente,
    resumir,
    enviarEmail: ({ usuario, senha, destinatario, ...dados }) =>
      enviar({ usuario, senha, destinatario, email: montarEmail(dados) }),
    config: () => settings.obter(),
    emit: enviarParaTela,
  });
}

function registrarIpc() {
  ipcMain.handle('buscar', (_e, { q, limit, offset }) => store.searchSessions(USER_ID, { search: q, limit, offset }));
  ipcMain.handle('conversa', (_e, id) => ({ sessao: store.getSession(id), falas: store.getTranscript(id) }));
  ipcMain.on('esconder', () => win?.hide());

  ipcMain.handle('config:ler', () => settings.paraTela());
  ipcMain.handle('config:salvar', (_e, novos) => {
    try { settings.salvar(novos); return { ok: true }; } catch (e) { return { ok: false, erro: e.message }; }
  });

  ipcMain.handle('reuniao:iniciar', async () => {
    try {
      await whisperServer.iniciar();
      reuniao = novaReuniao();
      reuniao.iniciar();
      return { ok: true };
    } catch (e) {
      reuniao = null;
      return { ok: false, erro: e.message };
    }
  });
  ipcMain.on('reuniao:audio', (_e, { fonte, buffer }) => reuniao?.receberAudio(fonte, new Int16Array(buffer)));
  ipcMain.handle('reuniao:parar', async () => {
    try { await reuniao?.parar(); return { ok: true }; } catch (e) { return { ok: false, erro: e.message }; }
  });
  ipcMain.handle('reuniao:reenviar', async (_e, id) => {
    await (reuniao ?? novaReuniao()).enviarPorEmail(id);
    return { ok: true };
  });
}

app.whenReady().then(() => {
  store = new ConversationStore(path.join(app.getPath('userData'), 'conversas.db'));
  seedIfEmpty(store, USER_ID);
  settings = new Settings(path.join(app.getPath('userData'), 'config.json'), cifra);
  whisperServer = new WhisperServer({ pasta: path.join(__dirname, 'whisper') });

  configurarCaptura();
  registrarIpc();
  criarJanela();
  win.once('ready-to-show', mostrar);

  if (!globalShortcut.register(ATALHO, alternar)) console.warn(`Não foi possível registrar o atalho ${ATALHO}`);
});

app.on('will-quit', () => {
  globalShortcut.unregisterAll();
  whisperServer?.parar();
  store?.close();
});

app.on('window-all-closed', () => app.quit());
