'use strict';

// Botão X no topo: pede confirmação antes de fechar o app.
(() => {
  const $ = (id) => document.getElementById(id);
  const fundo = $('dialogo-fechar');
  const msg = $('dialogo-msg');
  const confirmar = $('dialogo-confirmar');
  const cancelar = $('dialogo-cancelar');
  const textoMsg = msg.textContent;

  function abrir() {
    msg.textContent = textoMsg;
    msg.className = 'estado';
    confirmar.hidden = false;
    cancelar.textContent = 'Cancelar';
    fundo.classList.add('aberto');
    cancelar.focus();
  }

  function fechar() {
    fundo.classList.remove('aberto');
    $('fechar').focus();
  }

  $('fechar').addEventListener('click', abrir);
  cancelar.addEventListener('click', fechar);

  confirmar.addEventListener('click', async () => {
    const r = await window.busca.sair();
    if (r.ok) return; // o app encerra sozinho
    // Reunião em andamento: o app não fecha. Mostra o motivo no próprio aviso.
    msg.textContent = r.erro;
    msg.className = 'erro';
    confirmar.hidden = true;
    cancelar.textContent = 'Entendi';
  });

  // Esc fecha só o aviso; não deixa o atalho global esconder a janela.
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && fundo.classList.contains('aberto')) {
      e.stopPropagation();
      fechar();
    }
  }, true);
})();
