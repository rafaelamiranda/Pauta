'use strict';

// Abas e atalhos de teclado.
(() => {
  const abas = [...document.querySelectorAll('.aba')];
  const vistas = Object.fromEntries([...document.querySelectorAll('.vista')].map((v) => [v.id.replace('v-', ''), v]));

  function ir(nome) {
    abas.forEach((a) => a.setAttribute('aria-selected', String(a.dataset.vista === nome)));
    Object.entries(vistas).forEach(([k, v]) => v.classList.toggle('ativa', k === nome));
    if (nome === 'conversas') window.Conversas.focar();
    if (nome === 'pendencias') window.Pendencias.recarregar();
  }
  abas.forEach((a) => a.addEventListener('click', () => ir(a.dataset.vista)));

  document.addEventListener('keydown', (e) => {
    const naBusca = vistas.conversas.classList.contains('ativa');
    if (e.key === 'Escape') {
      if (naBusca && window.Conversas.voltar()) return;
      if (window.Reuniao.gravando()) return; // não esconde sem querer durante a reunião
      window.busca.esconder();
    } else if (naBusca && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      window.Conversas.navegar(e);
    }
  });

  window.busca.aoFocar(() => { if (vistas.conversas.classList.contains('ativa')) window.Conversas.focar(); });
  window.Conversas.focar();
})();
