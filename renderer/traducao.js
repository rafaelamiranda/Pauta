'use strict';

// Aba "Tradução": ouve o áudio num idioma e mostra a tradução na tela em tempo real.
(() => {
  const { el } = window.UI;
  const corpo = document.getElementById('traducao-corpo');
  const abaTraducao = document.getElementById('aba-traducao');
  const CHAVE = 'pauta.traducao';

  let ativa = false;
  let ocupado = false;
  let preferencias = { origem: 'en', destino: 'pt', entrada: 'sistema', original: true, enviarAoParar: false };
  try { Object.assign(preferencias, JSON.parse(localStorage.getItem(CHAVE) || '{}')); } catch { /* sem armazenamento: usa o padrão */ }
  const guardar = () => { try { localStorage.setItem(CHAVE, JSON.stringify(preferencias)); } catch { /* ignora */ } };

  const tela = {};
  const linhas = new Map(); // id da fala -> { raiz, traduzido, original }

  function campoSelect(rotulo, opcoes, valor, aoMudar) {
    const label = el('label', null, rotulo);
    const select = document.createElement('select');
    for (const [v, t] of opcoes) select.append(new Option(t, v));
    select.value = valor;
    select.addEventListener('change', () => aoMudar(select.value));
    label.append(select);
    return { label, select };
  }

  function mostrarErro(msg) {
    tela.estado.className = msg ? 'erro' : 'estado';
    tela.estado.textContent = msg || '';
  }

  function definirAtiva(valor) {
    ativa = valor;
    tela.origem.select.disabled = tela.destino.select.disabled = tela.entrada.select.disabled = tela.trocar.disabled = valor;
    tela.botao.textContent = valor ? 'Parar tradução' : 'Iniciar tradução';
    tela.botao.className = valor ? 'botao perigo' : 'botao';
    tela.ponto.hidden = !valor;
    abaTraducao.replaceChildren(...(valor ? [el('span', 'gravando-ponto'), 'Tradução'] : ['Tradução']));
    if (!valor) tela.nivel.style.width = '0';
  }

  function atualizarVazio() {
    tela.vazio.hidden = linhas.size > 0;
    tela.vazio.textContent = ativa ? 'Ouvindo... a tradução aparece aqui assim que alguém falar.' : 'Escolha os idiomas e toque em Iniciar tradução.';
  }

  async function alternar() {
    if (ocupado) return;
    ocupado = true;
    tela.botao.disabled = true;
    mostrarErro('');
    try {
      if (ativa) {
        tela.botao.textContent = 'Finalizando...';
        await window.busca.pararTraducao(preferencias.enviarAoParar);
        definirAtiva(false);
      } else {
        tela.botao.textContent = 'Iniciando...';
        const r = await window.busca.iniciarTraducao(preferencias.origem, preferencias.destino, preferencias.entrada);
        if (!r.ok) {
          mostrarErro(r.erro);
          definirAtiva(false);
        } else {
          definirAtiva(true);
          (r.avisos || []).forEach((a) => mostrarErro(a));
        }
      }
    } catch (e) {
      mostrarErro(String(e));
      definirAtiva(false);
    } finally {
      ocupado = false;
      tela.botao.disabled = false;
      atualizarVazio();
    }
  }

  function montar() {
    corpo.replaceChildren();
    corpo.append(el('h2', null, 'Tradução em tempo real'));
    corpo.append(el('p', 'nota', 'Ouve o áudio no idioma escolhido e mostra a tradução na tela, frase a frase. Nada é gravado nem salvo.'));

    const idiomas = el('div', 'trad-idiomas');
    tela.origem = campoSelect('Idioma do áudio', [['auto', 'Detectar automaticamente']], preferencias.origem, (v) => { preferencias.origem = v; guardar(); });
    tela.destino = campoSelect('Traduzir para', [], preferencias.destino, (v) => { preferencias.destino = v; guardar(); });
    tela.trocar = el('button', 'icone', '⇄');
    tela.trocar.type = 'button';
    tela.trocar.title = tela.trocar.ariaLabel = 'Trocar os idiomas';
    tela.trocar.addEventListener('click', () => {
      const { origem, destino } = preferencias;
      if (origem === 'auto') return;
      preferencias.origem = destino;
      preferencias.destino = origem;
      tela.origem.select.value = destino;
      tela.destino.select.value = origem;
      guardar();
    });
    idiomas.append(tela.origem.label, tela.trocar, tela.destino.label);
    corpo.append(idiomas);

    tela.entrada = campoSelect('Áudio a traduzir', [['sistema', 'Som do computador (vídeos, chamadas)'], ['mic', 'Microfone'], ['ambos', 'Som do computador e microfone']], preferencias.entrada, (v) => { preferencias.entrada = v; guardar(); });
    corpo.append(tela.entrada.label);

    tela.botao = el('button', 'botao', 'Iniciar tradução');
    tela.botao.addEventListener('click', alternar);
    tela.ponto = el('span', 'gravando-ponto');
    tela.ponto.hidden = true;
    const barra = el('div', 'barra');
    tela.nivel = el('i');
    barra.append(tela.nivel);
    const medidor = el('div', 'medidor');
    medidor.style.flex = '1';
    medidor.append(el('span', null, 'Áudio'), barra);
    const acoes = el('div', 'acoes');
    acoes.append(tela.ponto, tela.botao, medidor);
    corpo.append(acoes);

    tela.estado = el('p', 'estado');
    corpo.append(tela.estado);

    const email = el('div', 'acoes');
    const auto = document.createElement('input');
    auto.type = 'checkbox';
    auto.checked = preferencias.enviarAoParar;
    auto.addEventListener('change', () => { preferencias.enviarAoParar = auto.checked; guardar(); });
    const rotuloAuto = el('label');
    rotuloAuto.style.margin = '0';
    rotuloAuto.append(auto, 'Enviar por e-mail ao parar');
    tela.enviar = el('button', 'botao sec mini', 'Enviar por e-mail agora');
    tela.enviar.type = 'button';
    tela.enviar.addEventListener('click', async () => {
      const r = await window.busca.enviarTraducaoPorEmail();
      if (!r.ok) { tela.email.className = 'nota erro'; tela.email.textContent = r.erro; }
    });
    email.append(rotuloAuto, tela.enviar);
    tela.email = el('p', 'nota');
    corpo.append(email, tela.email);

    const cabecalho = el('div', 'acoes');
    const mostrar = document.createElement('input');
    mostrar.type = 'checkbox';
    mostrar.checked = preferencias.original;
    const rotulo = el('label');
    rotulo.style.margin = '0';
    rotulo.append(mostrar, 'Mostrar o texto original');
    const limpar = el('button', 'botao sec mini', 'Limpar');
    limpar.type = 'button';
    limpar.style.marginLeft = 'auto';
    limpar.addEventListener('click', () => { linhas.clear(); tela.lista.replaceChildren(); window.busca.limparTraducao(); atualizarVazio(); });
    cabecalho.append(rotulo, limpar);
    corpo.append(cabecalho);

    tela.lista = el('div', 'trad-lista');
    tela.lista.setAttribute('aria-live', 'polite');
    const aplicarOriginal = () => tela.lista.classList.toggle('sem-original', !mostrar.checked);
    mostrar.addEventListener('change', () => { preferencias.original = mostrar.checked; guardar(); aplicarOriginal(); });
    aplicarOriginal();
    tela.vazio = el('p', 'trad-vazio');
    corpo.append(tela.vazio, tela.lista);
    atualizarVazio();
  }

  async function carregarIdiomas() {
    const lista = await window.busca.idiomasTraducao();
    for (const { codigo, nome } of lista) {
      const n = nome.charAt(0).toUpperCase() + nome.slice(1);
      tela.origem.select.append(new Option(n, codigo));
      tela.destino.select.append(new Option(n, codigo));
    }
    tela.origem.select.value = preferencias.origem;
    tela.destino.select.value = preferencias.destino;
    // preferência antiga que não existe mais: volta ao padrão
    if (tela.origem.select.value !== preferencias.origem) preferencias.origem = tela.origem.select.value = 'en';
    if (tela.destino.select.value !== preferencias.destino) preferencias.destino = tela.destino.select.value = 'pt';
  }

  function perto() {
    return corpo.scrollHeight - corpo.scrollTop - corpo.clientHeight < 80;
  }

  window.busca.aoTraducaoFala(({ id, original, fonte }) => {
    if (!tela.lista || linhas.has(id)) return;
    const acompanhar = perto();
    const raiz = el('div', `trad-fala fonte-${fonte}`);
    const traduzido = el('div', 'traduzido pendente', '...');
    const orig = el('div', 'original', original);
    raiz.append(traduzido, orig);
    // a transcrição pode terminar fora de ordem: mantém a ordem em que as falas aconteceram
    let depois = null;
    for (const [outro, l] of linhas) if (outro > id && (!depois || outro < depois.id)) depois = { id: outro, raiz: l.raiz };
    tela.lista.insertBefore(raiz, depois ? depois.raiz : null);
    linhas.set(id, { raiz, traduzido });
    atualizarVazio();
    if (acompanhar) corpo.scrollTop = corpo.scrollHeight;
  });

  window.busca.aoTraduzida(({ id, traducao, erro, parcial }) => {
    const l = linhas.get(id);
    if (!l) return;
    const acompanhar = perto();
    l.traduzido.classList.remove('pendente');
    l.traduzido.classList.toggle('parcial', !!parcial);
    if (erro) {
      l.traduzido.classList.add('erro');
      l.traduzido.textContent = `Não foi possível traduzir: ${erro}`;
    } else {
      l.traduzido.textContent = traducao;
    }
    if (acompanhar) corpo.scrollTop = corpo.scrollHeight;
  });

  window.busca.aoTraducaoNivel(({ valor }) => { if (tela.nivel) tela.nivel.style.width = `${Math.round(valor * 100)}%`; });
  window.busca.aoTraducaoEmail((e) => {
    tela.email.className = e.estado === 'erro' ? 'nota erro' : 'nota';
    tela.email.textContent = e.estado === 'enviando' ? 'Enviando e-mail...' : e.estado === 'enviado' ? `Tradução enviada para ${e.para}.` : `Não foi possível enviar: ${e.mensagem}`;
  });
  window.busca.aoTraducaoAviso((msg) => mostrarErro(msg));

  montar();
  carregarIdiomas().catch((e) => mostrarErro(String(e)));
})();
