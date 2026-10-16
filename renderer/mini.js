'use strict';

// Mini player: a janela encolhe e mostra só a última frase transcrita e o botão de parar.
(() => {
  const $ = (id) => document.getElementById(id);
  const ponto = $('player-ponto');
  const tempo = $('player-tempo');
  const texto = $('player-texto');
  const parar = $('player-parar');

  let mini = false;
  let relogio = null;

  const hms = (ms) => {
    const s = Math.floor(ms / 1000);
    return [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60].map((n) => String(n).padStart(2, '0')).join(':');
  };

  function mostrarTexto(t, suave) {
    texto.textContent = t;
    texto.classList.toggle('vazio-p', suave);
  }

  async function definir(valor) {
    if (valor === mini) return;
    mini = valor;
    // Ao encolher, troca a tela antes de redimensionar; ao expandir, redimensiona antes.
    if (valor) document.body.classList.add('modo-mini');
    try {
      await window.busca.modoMini(valor);
    } catch (e) {
      mini = !valor;
      document.body.classList.toggle('modo-mini', mini);
      return;
    }
    if (!valor) {
      document.body.classList.remove('modo-mini');
      window.Conversas?.focar();
    }
  }

  function gravando(inicio) {
    clearInterval(relogio);
    ponto.hidden = tempo.hidden = parar.hidden = false;
    parar.disabled = false;
    tempo.textContent = '00:00:00';
    relogio = setInterval(() => { tempo.textContent = hms(Date.now() - inicio); }, 500);
    mostrarTexto('Aguardando a primeira fala...', true);
  }

  function finalizando(msg) {
    clearInterval(relogio);
    parar.disabled = true;
    mostrarTexto(msg, true);
  }

  function encerrou() {
    clearInterval(relogio);
    ponto.hidden = tempo.hidden = parar.hidden = true;
    mostrarTexto('Nenhuma reunião em andamento', true);
    definir(false); // o resumo aparece na janela cheia
  }

  window.busca.aoFalar(({ speaker, text }) => {
    if (!text?.trim()) return;
    mostrarTexto(`${window.UI.rotulo(speaker)}: ${text.trim()}`, false);
  });

  parar.addEventListener('click', () => window.Reuniao.finalizar());
  $('minimizar').addEventListener('click', () => definir(true));
  $('player-expandir').addEventListener('click', () => definir(false));

  window.Mini = { gravando, finalizando, encerrou, minimizado: () => mini };
})();
