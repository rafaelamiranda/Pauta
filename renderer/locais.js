'use strict';

// Ajustes > Modelos locais (opcional): baixar, escolher, testar e apagar modelos de voz e de análise.
(() => {
  const { el, confirmarDuplo } = window.UI;
  const raiz = document.getElementById('locais-corpo');

  let config = {};
  let modo = 'openai'; // openai | local | misto
  let itens = [];
  const vivos = new Map(); // id -> { barra, texto } do cartão que está baixando

  const tamanho = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1).replace('.', ',')} GB` : `${Math.round(b / 1e6)} MB`);

  function botao(texto, aoClicar, tipo = 'sec') {
    const b = el('button', `botao ${tipo} mini`, texto);
    b.type = 'button';
    b.addEventListener('click', aoClicar);
    return b;
  }

  function linhaProgresso(item) {
    const barra = document.createElement('progress');
    barra.max = 100;
    const texto = el('span', 'nota');
    const atualizar = (fase, baixado, total) => {
      if (total) barra.value = Math.round((baixado / total) * 100); // sem total, fica indeterminada
      texto.textContent = fase === 'programa'
        ? `Baixando o programa${total ? ` (${tamanho(baixado)} de ${tamanho(total)})` : '...'}`
        : `Baixando o modelo: ${tamanho(baixado)} de ${tamanho(total || item.tamanho)}`;
    };
    atualizar(item.fase, item.baixado, item.total);
    vivos.set(item.id, { atualizar });
    return [barra, texto];
  }

  function cartao(item, grupo) {
    const c = el('div', 'cartao-modelo');
    const topo = el('div', 'linha-form');
    const radio = document.createElement('input');
    radio.type = 'radio';
    radio.name = grupo;
    radio.value = item.id;
    radio.checked = (grupo === 'whisperLocal' ? config.whisperLocal : config.llmLocal) === item.id;
    radio.disabled = item.estado !== 'pronto';
    radio.setAttribute('aria-label', `Usar ${item.nome}`);
    topo.append(radio, el('strong', null, item.nome));
    if (item.recomendado) topo.append(el('span', 'nota', 'sugerido'));
    c.append(topo, el('p', 'nota', `${item.descricao} ${tamanho(item.tamanho)} em disco. Licença: ${item.licenca}.`));

    const acoes = el('div', 'linha-form');
    const msg = el('span', 'nota');
    const baixar = async () => {
      const r = await window.busca.baixarLocal(item.id);
      if (!r.ok) { msg.className = 'nota erro'; msg.textContent = r.erro; return; }
      recarregar();
    };
    if (item.estado === 'baixando') {
      acoes.append(...linhaProgresso(item), botao('Cancelar', () => window.busca.cancelarLocal(item.id)));
    } else if (item.estado === 'pronto') {
      msg.textContent = item.programaInstalado ? 'Baixado.' : 'Modelo baixado, mas falta o programa para o processamento escolhido.';
      acoes.append(msg);
      if (!item.programaInstalado) acoes.append(botao('Baixar o programa', baixar, ''));
      const testar = botao('Testar', async () => {
        testar.disabled = true;
        msg.className = 'nota';
        msg.textContent = 'Ligando o modelo (o primeiro uso é mais lento)...';
        const r = await window.busca.testarLocal(item.tipo);
        testar.disabled = false;
        msg.className = r.ok ? 'nota' : 'nota erro';
        msg.textContent = r.ok ? `Funcionando. Ligou e respondeu em ${r.segundos.toFixed(1).replace('.', ',')} s.` : r.erro;
      });
      const apagar = el('button', 'botao perigo mini', 'Apagar modelo');
      apagar.type = 'button';
      confirmarDuplo(apagar, async () => {
        const r = await window.busca.apagarLocal(item.id);
        if (r.ok) recarregar(); else { msg.className = 'nota erro'; msg.textContent = r.erro; }
      }, 'Confirmar: apagar do disco');
      acoes.append(testar, apagar);
    } else {
      if (item.estado === 'erro') { msg.className = 'nota erro'; msg.textContent = item.erro; }
      if (item.estado === 'parcial') msg.textContent = `Download parcial (${tamanho(item.baixado)}). Continua de onde parou.`;
      acoes.append(botao(item.estado === 'parcial' ? 'Continuar download' : item.estado === 'erro' ? 'Tentar de novo' : 'Baixar', baixar, ''), msg);
    }
    c.append(acoes);
    return c;
  }

  function grupo(titulo, descricao, chave, tipo, extra) {
    const g = el('div');
    g.append(el('h2', null, titulo));
    const label = el('label');
    const usar = document.createElement('input');
    usar.type = 'checkbox';
    usar.id = chave;
    usar.checked = Boolean(config[chave]);
    label.append(usar, descricao);
    label.hidden = modo !== 'misto'; // nos outros modos a escolha já foi feita lá em cima
    g.append(label);
    if (extra) g.append(extra);
    const radios = tipo === 'whisper' ? 'whisperLocal' : 'llmLocal';
    itens.filter((i) => i.tipo === tipo).forEach((i) => g.append(cartao(i, radios)));
    return g;
  }

  function seletorProcessamento() {
    const l = el('label', null, 'Processamento do modelo de análise');
    const sel = document.createElement('select');
    sel.id = 'aceleracao';
    sel.append(new Option('Processador (funciona em qualquer computador)', 'cpu'), new Option('Placa de vídeo (Vulkan): baixa um programa a mais', 'vulkan'));
    sel.value = config.aceleracao || 'cpu';
    sel.addEventListener('change', async () => {
      await window.busca.salvarConfig({ aceleracao: sel.value });
      config.aceleracao = sel.value;
      recarregar();
    });
    l.append(sel);
    return l;
  }

  function desenhar() {
    vivos.clear();
    raiz.replaceChildren(
      el('p', 'nota', 'Opcional. Com modelos locais, o áudio e o texto não saem do computador. Em troca: é preciso baixar arquivos grandes, o primeiro uso é mais lento porque o modelo carrega na memória, e a qualidade costuma ser menor que a da OpenAI, principalmente em resumo e pendências. Compare antes de usar numa reunião importante. Se o modelo local falhar, o app mostra o erro e você pode voltar para a OpenAI aqui mesmo.'),
      grupo('Transcrição no computador', 'Transcrever as falas aqui, sem enviar o áudio para a OpenAI', 'transcricaoLocal', 'whisper'),
      grupo('Análise no computador', 'Resumir e responder perguntas aqui, sem enviar texto para a OpenAI', 'analiseLocal', 'llm', seletorProcessamento()),
    );
  }

  async function recarregar() {
    itens = await window.busca.listarLocais();
    desenhar();
  }

  async function carregar(c) {
    config = c;
    modo = c.transcricaoLocal && c.analiseLocal ? 'local' : c.transcricaoLocal || c.analiseLocal ? 'misto' : 'openai';
    await recarregar();
    return modo;
  }

  /** Troca o modo escolhido em Ajustes, guardando o que já estava marcado nos cartões. */
  function definirModo(novo) {
    config = { ...config, ...valores() };
    modo = novo;
    desenhar();
  }

  window.busca.aoProgressoLocal((e) => {
    if (e.estado === 'baixando') return vivos.get(e.id)?.atualizar(e.fase, e.baixado, e.total);
    recarregar();
  });

  /** Valores dos campos desta seção, para o botão Salvar. */
  function valores() {
    const marcado = (nome) => document.querySelector(`input[name="${nome}"]:checked`)?.value;
    const v = modo === 'misto'
      ? { transcricaoLocal: document.getElementById('transcricaoLocal')?.checked ?? false, analiseLocal: document.getElementById('analiseLocal')?.checked ?? false }
      : { transcricaoLocal: modo === 'local', analiseLocal: modo === 'local' };
    if (marcado('whisperLocal')) v.whisperLocal = marcado('whisperLocal');
    if (marcado('llmLocal')) v.llmLocal = marcado('llmLocal');
    const acel = document.getElementById('aceleracao')?.value;
    if (acel) v.aceleracao = acel;
    return v;
  }

  window.ModelosLocais = { carregar, valores, definirModo };
})();
