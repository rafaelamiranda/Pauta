'use strict';

// Aba "Pendências": todas as pendências de todas as reuniões, com marcar como feita.
(() => {
  const { el } = window.UI;
  const corpo = document.getElementById('pendencias-corpo');
  let mostrarFeitas = false;
  let seq = 0;

  const data = (seg) => new Date(seg * 1000).toLocaleDateString('pt-BR', { day: '2-digit', month: 'short', year: 'numeric' });

  function linha(p) {
    const caixa = el('div', p.pendencia.feita ? 'pend feita' : 'pend');
    const marca = document.createElement('input');
    marca.type = 'checkbox';
    marca.checked = p.pendencia.feita;
    marca.setAttribute('aria-label', `Marcar como feita: ${p.pendencia.tarefa}`);
    marca.addEventListener('change', async () => {
      marca.disabled = true;
      const r = await window.busca.marcarPendencia(p.sessao_id, p.indice, marca.checked);
      if (!r.ok) {
        marca.checked = !marca.checked;
        marca.disabled = false;
        return;
      }
      recarregar();
    });
    const texto = el('div', 'texto');
    const extras = [p.pendencia.responsavel && `responsável: ${p.pendencia.responsavel}`, p.pendencia.prazo && `prazo: ${p.pendencia.prazo}`].filter(Boolean);
    texto.append(el('div', null, p.pendencia.tarefa));
    texto.append(el('div', 'origem', `${p.titulo || 'Reunião'}, ${data(p.data)}${extras.length ? ` (${extras.join(', ')})` : ''}`));
    caixa.append(marca, texto);
    return caixa;
  }

  async function recarregar() {
    const minha = ++seq;
    const lista = await window.busca.listarPendencias(mostrarFeitas);
    if (minha !== seq) return;
    corpo.replaceChildren(el('h2', null, 'Pendências de todas as reuniões'));
    const opcao = el('label');
    opcao.style.marginTop = '0';
    const caixaFeitas = document.createElement('input');
    caixaFeitas.type = 'checkbox';
    caixaFeitas.checked = mostrarFeitas;
    caixaFeitas.addEventListener('change', () => { mostrarFeitas = caixaFeitas.checked; recarregar(); });
    opcao.append(caixaFeitas, 'Mostrar também as feitas');
    corpo.append(opcao);
    if (!lista.length) {
      corpo.append(el('p', 'estado', mostrarFeitas ? 'Nenhuma pendência registrada.' : 'Nada em aberto.'));
      return;
    }
    lista.forEach((p) => corpo.append(linha(p)));
  }

  window.Pendencias = { recarregar };
  recarregar();
})();
