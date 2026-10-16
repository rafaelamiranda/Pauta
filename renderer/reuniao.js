'use strict';

// Aba "Reunião": material de apoio, captura do áudio, transcrição ao vivo e resumo no final.
(() => {
  const { el, renderResumo, caixaPergunta, confirmarDuplo, rotulo } = window.UI;
  const corpo = document.getElementById('reuniao-corpo');
  const abaReuniao = document.getElementById('aba-reuniao');
  const LIMITE_ARQUIVO = 25 * 1024 * 1024;

  let fase = 'inicio'; // inicio | iniciando | gravando | finalizando | final
  let gravando = false; // true enquanto a captura nativa está ligada
  let relogio = null;
  let inicio = 0;
  let ao_vivo = null; // elemento com as falas
  let estadoEmail = null;
  let barras = null; // { mic, sistema } durante a gravação

  const hms = (ms) => {
    const s = Math.floor(ms / 1000);
    return [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60].map((n) => String(n).padStart(2, '0')).join(':');
  };

  const contagem = (t) => (t.trim() ? `${t.length.toLocaleString('pt-BR')} caracteres de texto colado` : 'Nenhum texto colado.');
  const botaoMini = (texto, tipo = 'sec') => el('button', `botao ${tipo} mini`, texto);

  /** Lista de documentos anexados, com botão de remover. */
  function listaDocumentos(docs, aoMudar) {
    const ul = el('ul', 'lista-doc');
    if (!docs.length) return el('p', 'nota', 'Nenhum documento anexado.');
    for (const d of docs) {
      const li = el('li');
      li.append(el('span', 'nome', `${d.origem === 'drive' ? 'Drive' : 'Arquivo'}: ${d.nome} (${d.caracteres.toLocaleString('pt-BR')} caracteres)`));
      const remover = botaoMini('Remover');
      remover.addEventListener('click', async () => {
        const r = await window.busca.removerDocumento(d.id);
        if (r.ok) aoMudar(r.documentos);
      });
      li.append(remover);
      ul.append(li);
    }
    return ul;
  }

  /** Painel de busca no Google Drive. */
  function painelDrive(aoAnexar) {
    const painel = el('div');
    painel.hidden = true;
    const linha = el('div', 'linha-form');
    const campo = document.createElement('input');
    campo.type = 'text';
    campo.placeholder = 'Nome do arquivo, texto dentro dele ou link do Drive';
    campo.setAttribute('aria-label', 'Buscar no Google Drive');
    const buscar = botaoMini('Buscar no Drive', 'sec');
    const msg = el('p', 'nota');
    const resultados = el('ul', 'lista-doc');
    linha.append(campo, buscar);
    painel.append(linha, msg, resultados);

    async function procurar() {
      resultados.replaceChildren();
      msg.className = 'nota';
      msg.textContent = 'Buscando...';
      const r = await window.busca.buscarDrive(campo.value);
      if (!r.ok) {
        msg.className = 'nota erro';
        msg.textContent = r.erro;
        return;
      }
      msg.textContent = r.arquivos.length ? (campo.value.trim() ? 'Escolha o arquivo para anexar.' : 'Arquivos mais recentes. Escolha um para anexar.') : 'Nada encontrado.';
      for (const a of r.arquivos) {
        const li = el('li');
        li.append(el('span', 'nome', a.nome));
        const b = botaoMini('Anexar', '');
        b.addEventListener('click', async () => {
          b.disabled = true;
          b.textContent = 'Baixando...';
          const res = await window.busca.anexarDoDrive(a.id);
          if (res.ok) {
            b.textContent = 'Anexado';
            aoAnexar(res.documentos);
          } else {
            b.disabled = false;
            b.textContent = 'Anexar';
            msg.className = 'nota erro';
            msg.textContent = `${a.nome}: ${res.erro}`;
          }
        });
        li.append(b);
        resultados.append(li);
      }
    }
    buscar.addEventListener('click', procurar);
    campo.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); procurar(); } });

    painel.abrir = async () => {
      const e = await window.busca.estadoDrive();
      painel.hidden = !painel.hidden;
      if (painel.hidden) return;
      if (!e.conectado) {
        msg.className = 'nota erro';
        msg.textContent = e.configurado ? 'Conecte o Google Drive na aba Ajustes.' : 'Configure o Google Drive na aba Ajustes (ID e segredo do cliente) e conecte.';
        return;
      }
      campo.focus();
      if (!resultados.children.length) procurar();
    };
    return painel;
  }

  /** Materiais salvos por cliente ou projeto. */
  function blocoSalvos(aoCarregar) {
    const caixa = el('div');
    const linha = el('div', 'linha-form');
    const select = document.createElement('select');
    select.setAttribute('aria-label', 'Material salvo');
    const carregar = botaoMini('Carregar');
    const excluir = botaoMini('Excluir', 'perigo');
    linha.append(select, carregar, excluir);
    const linhaSalvar = el('div', 'linha-form');
    const nome = document.createElement('input');
    nome.type = 'text';
    nome.placeholder = 'Nome do cliente ou projeto';
    nome.setAttribute('aria-label', 'Nome para salvar o material');
    const salvar = botaoMini('Salvar material atual');
    linhaSalvar.append(nome, salvar);
    const msg = el('p', 'nota');
    caixa.append(linha, linhaSalvar, msg);

    let lista = [];
    async function recarregar(escolher) {
      lista = await window.busca.listarMateriais();
      select.replaceChildren(new Option(lista.length ? 'Escolha um material salvo...' : 'Nenhum material salvo ainda', ''));
      for (const m of lista) select.append(new Option(`${m.nome} (${m.documentos} doc.)`, m.id));
      select.value = escolher || '';
      carregar.disabled = excluir.disabled = !select.value;
    }
    select.addEventListener('change', () => {
      const m = lista.find((x) => x.id === select.value);
      carregar.disabled = excluir.disabled = !m;
      if (m) nome.value = m.nome;
    });
    carregar.addEventListener('click', async () => {
      const r = await window.busca.carregarMaterial(select.value);
      msg.className = r.ok ? 'nota' : 'nota erro';
      msg.textContent = r.ok ? 'Material carregado.' : r.erro;
      if (r.ok) aoCarregar(r);
    });
    confirmarDuplo(excluir, async () => {
      const r = await window.busca.excluirMaterial(select.value);
      msg.className = r.ok ? 'nota' : 'nota erro';
      msg.textContent = r.ok ? 'Material excluído.' : r.erro;
      if (r.ok) { nome.value = ''; recarregar(); }
    }, 'Confirmar exclusão');
    salvar.addEventListener('click', async () => {
      const existente = lista.find((m) => m.nome.toLowerCase() === nome.value.trim().toLowerCase());
      const r = await window.busca.guardarMaterial(nome.value, existente?.id);
      msg.className = r.ok ? 'nota' : 'nota erro';
      msg.textContent = r.ok ? (existente ? 'Material atualizado.' : 'Material salvo.') : r.erro;
      if (r.ok) recarregar(r.id);
    });
    recarregar();
    return caixa;
  }

  /** Material de apoio: texto colado, documentos (arquivo ou Google Drive) e materiais salvos. */
  async function blocoMaterial() {
    const caixa = el('div');
    const ta = document.createElement('textarea');
    ta.rows = 6;
    ta.placeholder = 'Cole aqui textos, pauta, anotações...';
    ta.setAttribute('aria-label', 'Material de apoio');
    ta.value = await window.busca.lerMaterial();
    const estado = el('p', 'nota', contagem(ta.value));
    let espera;
    ta.addEventListener('input', () => {
      clearTimeout(espera);
      estado.textContent = 'Salvando...';
      espera = setTimeout(async () => {
        const r = await window.busca.salvarMaterial(ta.value);
        estado.textContent = r.ok ? contagem(ta.value) : `Erro ao salvar: ${r.erro}`;
      }, 600);
    });

    const docs = el('div');
    const mostrarDocs = (lista) => docs.replaceChildren(listaDocumentos(lista, mostrarDocs));
    mostrarDocs(await window.busca.listarDocumentos());

    const drive = painelDrive(mostrarDocs);
    const acoes = el('div', 'acoes');
    const anexar = botaoMini('Anexar arquivo (PDF, Word, TXT)');
    const entrada = document.createElement('input');
    entrada.type = 'file';
    entrada.accept = '.pdf,.docx,.txt,.md,.csv,.tsv,.json,.html,.htm';
    entrada.multiple = true;
    entrada.hidden = true;
    const aviso = el('span', 'nota');
    anexar.addEventListener('click', () => entrada.click());
    entrada.addEventListener('change', async () => {
      for (const arq of entrada.files) {
        if (arq.size > LIMITE_ARQUIVO) {
          aviso.className = 'nota erro';
          aviso.textContent = `${arq.name} passa de ${LIMITE_ARQUIVO / 1024 / 1024} MB.`;
          continue;
        }
        aviso.className = 'nota';
        aviso.textContent = `Lendo ${arq.name}...`;
        const r = await window.busca.anexarArquivo(arq.name, new Uint8Array(await arq.arrayBuffer()));
        if (r.ok) {
          aviso.textContent = `${arq.name} anexado.`;
          mostrarDocs(r.documentos);
        } else {
          aviso.className = 'nota erro';
          aviso.textContent = r.erro;
        }
      }
      entrada.value = '';
    });
    const botaoDrive = botaoMini('Anexar do Google Drive');
    botaoDrive.addEventListener('click', () => drive.abrir());
    acoes.append(anexar, botaoDrive, aviso, entrada);

    const salvos = blocoSalvos(async (r) => {
      ta.value = r.texto;
      estado.textContent = contagem(ta.value);
      mostrarDocs(r.documentos);
    });
    const det = el('details');
    det.append(el('summary', null, 'Materiais salvos (cliente ou projeto)'), salvos);

    caixa.append(ta, estado, el('h2', null, 'Documentos'), docs, acoes, drive, det);
    return caixa;
  }

  function telaInicial(erro) {
    fase = 'inicio';
    barras = null;
    corpo.replaceChildren();
    corpo.append(el('h2', null, 'Nova reunião'));
    corpo.append(el('p', null, 'O app vai ouvir o seu microfone e o som do computador e transcrever em tempo real. Ao finalizar, mostra o resumo e as pendências e envia a transcrição por e-mail.'));
    corpo.append(el('p', 'nota', 'Avise os participantes de que a reunião está sendo gravada e transcrita. Ctrl+Shift+R inicia e finaliza a reunião, mesmo com a janela escondida.'));
    const p = window.Ajustes.pronto;
    if (!p.resumo) corpo.append(el('p', 'nota erro', 'Falta a chave da OpenAI nos Ajustes. Ela é necessária para transcrever e resumir.'));
    if (!p.email) corpo.append(el('p', 'nota', 'E-mail não configurado nos Ajustes: a transcrição não será enviada.'));
    if (erro) corpo.append(el('p', 'erro', erro));
    corpo.append(el('h2', null, 'Material de apoio'));
    corpo.append(el('p', 'nota', 'Cole texto ou anexe documentos (arquivo ou Google Drive). O app usa isso, junto com a reunião, para responder suas perguntas, e a resposta diz de qual documento veio.'));
    const slot = el('div');
    corpo.append(slot);
    blocoMaterial().then((b) => slot.append(b));
    corpo.append(el('h2', null, 'Perguntar'), caixaPergunta(null));
    const b = el('button', 'botao', 'Iniciar reunião');
    b.addEventListener('click', () => iniciar());
    const acoes = el('div', 'acoes');
    acoes.append(b);
    corpo.append(acoes);
  }

  async function iniciar(porAtalho = false) {
    if (fase === 'iniciando' || fase === 'gravando' || fase === 'finalizando') return;
    fase = 'iniciando';
    corpo.replaceChildren(el('p', 'estado', 'Iniciando o motor de transcrição...'));
    await window.Ajustes.recarregar();
    const r = await window.busca.iniciarReuniao();
    if (!r.ok) {
      telaInicial(r.erro);
      if (porAtalho) window.busca.mostrarJanela();
      return;
    }
    gravando = true;
    window.Mini.gravando(Date.now());
    telaGravando(r.avisos || []);
  }

  function medidor(rotulo) {
    const linha = el('div', 'medidor');
    const barra = el('div', 'barra');
    const preenchido = el('i');
    barra.append(preenchido);
    linha.append(el('span', null, rotulo), barra);
    linha.preenchido = preenchido;
    return linha;
  }

  function telaGravando(avisos) {
    fase = 'gravando';
    inicio = Date.now();
    abaReuniao.replaceChildren(el('span', 'gravando-ponto'), 'Reunião');
    corpo.replaceChildren();
    const topo = el('div', 'acoes');
    const tempo = el('strong', null, '00:00:00');
    const parar = el('button', 'botao perigo', 'Finalizar reunião');
    parar.addEventListener('click', finalizar);
    topo.append(el('span', 'gravando-ponto'), tempo, parar);
    corpo.append(topo);
    barras = { mic: medidor('Microfone'), sistema: medidor('Som do computador') };
    corpo.append(barras.mic, barras.sistema);
    avisos.forEach((a) => corpo.append(el('p', 'nota erro', a)));
    corpo.append(el('h2', null, 'Perguntar'), caixaPergunta(null));
    const det = el('details');
    det.append(el('summary', null, 'Material de apoio'));
    const slot = el('div');
    det.append(slot);
    blocoMaterial().then((b) => slot.append(b));
    corpo.append(det);
    ao_vivo = el('div');
    corpo.append(el('h2', null, 'Transcrição ao vivo'), ao_vivo);
    relogio = setInterval(() => { tempo.textContent = hms(Date.now() - inicio); }, 500);
  }

  async function finalizar() {
    if (fase !== 'gravando') return;
    fase = 'finalizando';
    clearInterval(relogio);
    barras = null;
    corpo.replaceChildren(el('p', 'estado', 'Finalizando a transcrição...'));
    gravando = false;
    abaReuniao.textContent = 'Reunião';
    window.Mini.finalizando('Finalizando a transcrição...');
    await window.busca.pararReuniao(); // resumo e e-mail chegam pelos eventos abaixo
  }

  window.busca.aoAtalhoReuniao(() => {
    if (fase === 'gravando') finalizar();
    else if (fase === 'inicio' || fase === 'final') iniciar(true);
  });

  window.busca.aoNivel(({ fonte, valor }) => {
    const b = barras?.[fonte];
    if (b) b.preenchido.style.width = `${Math.round(valor * 100)}%`;
  });

  window.busca.aoEstado(({ fase: f }) => {
    if (f === 'resumindo') {
      corpo.replaceChildren(el('p', 'estado', 'Gerando o resumo...'));
      window.Mini.finalizando('Gerando o resumo...');
    }
  });

  window.busca.aoFalar(({ speaker, text }) => {
    if (!ao_vivo) return;
    const p = el('div', speaker === 'ME' ? 'fala' : 'fala outros');
    p.append(el('b', null, rotulo(speaker)), text);
    ao_vivo.append(p);
    corpo.scrollTop = corpo.scrollHeight;
  });

  window.busca.aoAviso((msg) => ao_vivo?.append(el('p', 'nota erro', msg)));

  window.busca.aoFinal((f) => {
    fase = 'final';
    window.Mini.encerrou();
    ao_vivo = null;
    corpo.replaceChildren();
    if (f.vazia) {
      corpo.append(el('h2', null, 'Reunião encerrada'), el('p', null, 'Nenhuma fala foi captada.'));
    } else {
      corpo.append(el('h2', null, f.titulo));
      renderResumo(corpo, { ...f.resumo, erroResumo: f.erroResumo });
      corpo.append(el('h2', null, 'Perguntar'), caixaPergunta(f.id));
      corpo.append(el('h2', null, 'E-mail'));
      estadoEmail = el('p', 'estado', window.Ajustes.pronto.email ? 'Aguardando envio...' : 'Não enviado: e-mail não configurado nos Ajustes.');
      corpo.append(estadoEmail);
    }
    const nova = el('button', 'botao sec', 'Nova reunião');
    nova.addEventListener('click', () => telaInicial());
    const acoes = el('div', 'acoes');
    acoes.append(nova);
    corpo.append(acoes);
    window.Conversas.recarregar();
    window.Pendencias?.recarregar();
  });

  window.busca.aoEmail((e) => {
    if (!estadoEmail?.isConnected) return;
    estadoEmail.className = e.estado === 'erro' ? 'erro' : 'estado';
    estadoEmail.textContent = e.estado === 'enviando' ? 'Enviando e-mail...' : e.estado === 'enviado' ? `Transcrição enviada para ${e.para}.` : `Não foi possível enviar: ${e.mensagem}`;
  });

  window.Reuniao = { telaInicial, finalizar, gravando: () => gravando };
  window.Ajustes.recarregar().then(() => telaInicial());
})();
