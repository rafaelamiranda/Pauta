'use strict';

// Aba "Conversas": busca, lista e detalhe de uma conversa.
(() => {
  const PAGINA = 20;
  const campo = document.getElementById('q');
  const lista = document.getElementById('lista');
  const detalhe = document.getElementById('detalhe');
  const contagem = document.getElementById('contagem');

  let termo = '';
  let carregados = 0;
  let seq = 0; // descarta respostas antigas quando o usuário digita rápido
  let espera;
  let estadoEmail = null; // linha de status do e-mail no detalhe aberto

  window.busca.aoEmail((e) => {
    if (!estadoEmail?.isConnected) return;
    estadoEmail.textContent = e.estado === 'enviado' ? `Enviado para ${e.para}` : e.estado === 'erro' ? `Erro: ${e.mensagem}` : 'Enviando...';
  });

  function el(tag, classe, texto) {
    const n = document.createElement(tag);
    if (classe) n.className = classe;
    if (texto != null) n.textContent = texto;
    return n;
  }

  /** Desenha resumo, decisões, pendências e dúvidas (sem innerHTML). */
  function renderResumo(alvo, { resumo, decisoes = [], pendencias = [], duvidas = [], erroResumo }) {
    const lista = (titulo, itens, vazio) => {
      alvo.append(el('h2', null, titulo));
      if (!itens.length) return alvo.append(el('p', 'estado', vazio));
      const ul = el('ul');
      itens.forEach((t) => ul.append(el('li', null, t)));
      alvo.append(ul);
    };
    if (resumo) {
      alvo.append(el('h2', null, 'Resumo'), el('p', null, resumo));
      if (decisoes.length) lista('Decisões', decisoes, '');
    } else if (erroResumo) {
      alvo.append(el('h2', null, 'Resumo'), el('p', 'erro', `Não foi possível gerar o resumo: ${erroResumo}`));
    }
    lista(
      'Pendências',
      pendencias.map((p) => `${p.tarefa}${p.responsavel ? ` (responsável: ${p.responsavel})` : ''}${p.prazo ? ` (prazo: ${p.prazo})` : ''}`),
      resumo ? 'Nada ficou pendente.' : 'Sem resumo, não dá para listar pendências.'
    );
    if (duvidas.length) lista('Dúvidas em aberto', duvidas, '');
  }

  const PEDIDOS = {
    responder: 'O que devo responder agora? Com base no que os outros disseram por último na reunião e no material, escreva a fala pronta em primeira pessoa.',
    perdi: 'O que eu perdi? Resuma só os últimos 5 minutos da reunião: o que foi dito, decidido ou pedido.',
  };

  /** Copia o texto para a área de transferência e devolve se deu certo. */
  async function copiar(texto) {
    try {
      await navigator.clipboard.writeText(texto);
      return true;
    } catch {
      return false;
    }
  }

  /** Botão que só executa no segundo clique (em até 4 segundos), sem janela de confirmação. */
  function confirmarDuplo(botao, acao, textoConfirma = 'Clique de novo para confirmar') {
    const original = botao.textContent;
    let espera;
    const voltar = () => { clearTimeout(espera); botao.textContent = original; botao.dataset.armado = ''; };
    botao.addEventListener('click', () => {
      if (botao.dataset.armado) {
        voltar();
        acao();
        return;
      }
      botao.dataset.armado = '1';
      botao.textContent = textoConfirma;
      espera = setTimeout(voltar, 4000);
    });
  }

  /** Caixa "Pergunte sobre o material e a reunião". `id` = conversa; null = reunião em andamento. */
  function caixaPergunta(id) {
    const caixa = el('div');
    const linha = el('div', 'pergunta');
    const campoP = el('input');
    campoP.type = 'text';
    campoP.placeholder = 'Pergunte sobre o material e a reunião';
    campoP.setAttribute('aria-label', 'Pergunta');
    const botao = el('button', 'botao', 'Perguntar');
    const resp = el('div', 'resposta');
    resp.setAttribute('aria-live', 'polite');
    const voltarNuvem = el('button', 'botao sec mini', 'Usar a OpenAI nas próximas respostas');
    voltarNuvem.hidden = true;
    voltarNuvem.addEventListener('click', async () => {
      await window.busca.salvarConfig({ analiseLocal: false });
      await window.Ajustes.recarregar();
      voltarNuvem.hidden = true;
      resp.textContent = 'Pronto: as próximas respostas usam a OpenAI. Pergunte de novo.';
    });
    const copia = el('button', 'botao sec mini', 'Copiar resposta');
    copia.hidden = true;
    async function enviar(texto) {
      const q = (typeof texto === 'string' ? texto : campoP.value).trim();
      if (!q) return;
      botao.disabled = true;
      copia.hidden = true;
      resp.className = 'resposta estado';
      resp.textContent = 'Pensando...';
      const r = await window.busca.perguntar(q, id);
      botao.disabled = false;
      resp.className = r.ok ? 'resposta' : 'resposta erro';
      resp.textContent = r.ok ? r.resposta : r.erro;
      copia.hidden = !r.ok;
      copia.textContent = 'Copiar resposta';
      voltarNuvem.hidden = r.ok || !window.Ajustes.pronto.analiseLocal;
    }
    botao.addEventListener('click', () => enviar());
    campoP.addEventListener('keydown', (e) => { if (e.key === 'Enter') enviar(); });
    copia.addEventListener('click', async () => { copia.textContent = (await copiar(resp.textContent)) ? 'Copiado' : 'Não foi possível copiar'; });
    linha.append(campoP, botao);
    const atalhos = el('div', 'atalhos');
    const pedido = (rotulo, texto) => {
      const b = el('button', 'botao sec mini', rotulo);
      b.addEventListener('click', () => { campoP.value = texto; enviar(texto); });
      atalhos.append(b);
    };
    pedido('O que respondo?', PEDIDOS.responder);
    if (id === null) pedido('O que eu perdi?', PEDIDOS.perdi);
    caixa.append(linha, atalhos, resp, copia, voltarNuvem);
    return caixa;
  }

  window.UI = { el, renderResumo, caixaPergunta, confirmarDuplo, copiar };

  /** Converte "[palavra]" do trecho em <mark>, sem usar innerHTML. */
  function comDestaque(texto) {
    const frag = document.createDocumentFragment();
    for (const parte of texto.split(/(\[[^\]]*\])/)) {
      if (parte.startsWith('[') && parte.endsWith(']')) frag.append(el('mark', null, parte.slice(1, -1)));
      else frag.append(parte);
    }
    return frag;
  }

  const formatarData = (seg) => new Date(seg * 1000).toLocaleDateString('pt-BR', { day: '2-digit', month: 'short', year: 'numeric' });

  let menu = null;

  function fecharMenu() {
    menu?.remove();
    menu = null;
  }

  /** Menu do botão direito numa conversa da lista: só "Apagar", com confirmação no segundo clique. */
  function menuApagar(x, y, id) {
    fecharMenu();
    menu = el('div', 'menu-contexto');
    menu.setAttribute('role', 'menu');
    const apagar = el('button', 'perigo', 'Apagar conversa');
    apagar.type = 'button';
    apagar.setAttribute('role', 'menuitem');
    apagar.addEventListener('click', async () => {
      if (!apagar.dataset.armado) {
        apagar.dataset.armado = '1';
        apagar.textContent = 'Confirmar: apagar para sempre';
        return;
      }
      const r = await window.busca.apagarConversa(id);
      fecharMenu();
      if (!r.ok) { contagem.textContent = `Erro: ${r.erro}`; return; }
      buscar(true);
      window.Pendencias?.recarregar();
    });
    menu.append(apagar);
    document.body.append(menu);
    // mantém o menu dentro da janela
    menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - menu.offsetWidth - 4))}px`;
    menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - menu.offsetHeight - 4))}px`;
    apagar.focus();
  }

  document.addEventListener('click', (e) => { if (menu && !menu.contains(e.target)) fecharMenu(); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && menu) { fecharMenu(); e.stopPropagation(); } }, true);
  window.addEventListener('blur', fecharMenu);

  function desenhar(s) {
    const item = el('div', 'item');
    item.tabIndex = 0;
    item.setAttribute('role', 'listitem');
    const linha = el('div', 'linha');
    linha.append(el('span', 'assunto', s.subject || 'Sem assunto'), el('span', 'data', formatarData(s.ended_at ?? s.started_at)));
    item.append(linha);
    if (s.overview) item.append(el('div', 'resumo', s.overview));
    if (s.snippet) {
      const t = el('div', 'trecho');
      t.append(comDestaque(s.snippet));
      item.append(t);
    }
    const abrir = () => abrirDetalhe(s.id);
    item.addEventListener('click', abrir);
    item.addEventListener('keydown', (e) => { if (e.key === 'Enter') abrir(); });
    item.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      menuApagar(e.clientX, e.clientY, s.id);
    });
    return item;
  }

  async function buscar(reiniciar) {
    const minha = ++seq;
    if (reiniciar) carregados = 0;
    const { sessions, total } = await window.busca.buscar(termo, PAGINA, carregados);
    if (minha !== seq) return;

    if (reiniciar) lista.replaceChildren();
    lista.querySelector('.mais')?.remove();
    sessions.forEach((s) => lista.append(desenhar(s)));
    carregados += sessions.length;

    if (total === 0) lista.replaceChildren(el('div', 'vazio', termo ? `Nada encontrado para "${termo}"` : 'Nenhuma conversa ainda'));
    else if (carregados < total) {
      const b = el('button', 'mais', 'Carregar mais');
      b.addEventListener('click', () => buscar(false));
      lista.append(b);
    }
    contagem.textContent = total === 1 ? '1 conversa' : `${total} conversas`;
  }

  async function abrirDetalhe(id) {
    const { sessao, falas } = await window.busca.conversa(id);
    detalhe.replaceChildren();
    const voltar = el('button', 'botao sec', 'Voltar');
    voltar.addEventListener('click', fecharDetalhe);
    detalhe.append(voltar, el('h2', null, sessao.subject || 'Sem assunto'));
    renderResumo(detalhe, { resumo: sessao.overview, pendencias: sessao.pending });

    detalhe.append(el('h2', null, 'Perguntar'), caixaPergunta(id));
    detalhe.append(el('h2', null, 'Transcrição'));
    if (!falas.length) detalhe.append(el('p', 'estado', 'Sem transcrição.'));
    for (const f of falas) {
      const p = el('div', f.speaker === 'ME' ? 'fala' : 'fala outros');
      p.append(el('b', null, f.speaker === 'ME' ? 'Eu' : 'Outros'), f.text);
      detalhe.append(p);
    }
    const exportar = el('div', 'acoes');
    const copiarBt = el('button', 'botao sec', 'Copiar');
    const salvarBt = el('button', 'botao sec', 'Salvar em Downloads (.md)');
    const apagarBt = el('button', 'botao perigo', 'Apagar esta conversa');
    const msg = el('span', 'estado');
    const dizer = (r, ok) => { msg.className = r.ok ? 'estado' : 'erro'; msg.textContent = r.ok ? ok : `Erro: ${r.erro}`; };
    copiarBt.addEventListener('click', async () => {
      const r = await window.busca.textoDaConversa(id);
      dizer(r.ok && (await copiar(r.texto)) ? { ok: true } : { ok: false, erro: r.erro || 'não foi possível copiar' }, 'Copiado.');
    });
    salvarBt.addEventListener('click', async () => {
      const r = await window.busca.salvarConversaEmArquivo(id);
      dizer(r, r.ok ? `Salvo em ${r.caminho}` : '');
    });
    confirmarDuplo(apagarBt, async () => {
      const r = await window.busca.apagarConversa(id);
      if (!r.ok) return dizer(r, '');
      fecharDetalhe();
      buscar(true);
      window.Pendencias?.recarregar();
    }, 'Confirmar: apagar para sempre');
    exportar.append(copiarBt, salvarBt, apagarBt, msg);
    voltar.after(exportar);
    if (falas.length) {
      const enviar = el('button', 'botao sec', 'Enviar por e-mail');
      const estado = el('span', 'estado');
      enviar.addEventListener('click', async () => {
        enviar.disabled = true;
        estado.textContent = 'Enviando...';
        await window.busca.reenviarEmail(id); // o resultado chega por aoEmail
        enviar.disabled = false;
      });
      estadoEmail = estado;
      const acoes = el('div', 'acoes');
      acoes.append(enviar, estado);
      detalhe.append(acoes);
    }
    lista.hidden = true;
    detalhe.hidden = false;
  }

  function fecharDetalhe() {
    detalhe.hidden = true;
    lista.hidden = false;
    campo.focus();
  }

  campo.addEventListener('input', () => {
    clearTimeout(espera);
    espera = setTimeout(() => {
      termo = campo.value.trim();
      buscar(true);
    }, 150);
  });

  window.Conversas = {
    recarregar: () => buscar(true),
    focar: () => { campo.focus(); campo.select(); },
    /** Esc na aba: devolve true se tratou (detalhe aberto ou busca preenchida). */
    voltar() {
      if (!detalhe.hidden) { fecharDetalhe(); return true; }
      if (campo.value) { campo.value = ''; termo = ''; buscar(true); return true; }
      return false;
    },
    navegar(e) {
      const itens = [...lista.querySelectorAll('.item')];
      if (!itens.length || lista.hidden) return;
      e.preventDefault();
      const i = itens.indexOf(document.activeElement);
      const prox = e.key === 'ArrowDown' ? i + 1 : i - 1;
      if (prox < 0) campo.focus();
      else itens[Math.min(prox, itens.length - 1)].focus();
    },
  };

  buscar(true);
})();
