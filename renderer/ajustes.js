'use strict';

// Aba "Ajustes": OpenAI (chave ou conta do ChatGPT), Gmail, áudio, Google Drive e dados.
(() => {
  const { confirmarDuplo } = window.UI;
  const form = document.getElementById('form-ajustes');
  const estado = document.getElementById('ajustes-estado');
  const campos = ['openaiKey', 'chatgptModel', 'openaiModel', 'transcricaoModel', 'smtpUser', 'smtpPass', 'destinatario', 'idioma', 'driveClientId', 'driveClientSecret'];
  const segredos = ['openaiKey', 'smtpPass', 'driveClientSecret'];
  const $ = (id) => document.getElementById(id);

  // Preços de referência em US$ (fontes de terceiros, sem conferência na página oficial). Análise: por 1 milhão de tokens. Transcrição: por minuto.
  const MODELOS_ANALISE = [
    { id: 'gpt-4o-mini', nome: 'GPT-4o mini (padrão, o mais barato)', entrada: 0.15, saida: 0.60 },
    { id: 'gpt-4.1-mini', nome: 'GPT-4.1 mini', entrada: 0.40, saida: 1.60 },
    { id: 'gpt-4.1', nome: 'GPT-4.1', entrada: 2.00, saida: 8.00 },
    { id: 'gpt-4o', nome: 'GPT-4o', entrada: 2.50, saida: 10.00 },
  ];
  const MODELOS_TRANSCRICAO = [
    { id: 'gpt-4o-mini-transcribe', nome: 'GPT-4o mini transcribe (o mais barato)', minuto: 0.003 },
    { id: 'gpt-4o-transcribe', nome: 'GPT-4o transcribe (padrão)', minuto: 0.006 },
    { id: 'whisper-1', nome: 'Whisper-1', minuto: 0.006 },
  ];
  // Modelos oferecidos no login do ChatGPT. O uso conta no limite da assinatura, não em dólar. Quais existem depende do plano da conta.
  const MODELOS_CHATGPT = [
    { id: 'gpt-5', nome: 'GPT-5 (padrão)' },
    { id: 'gpt-5-codex', nome: 'GPT-5 Codex' },
    { id: 'gpt-5.1', nome: 'GPT-5.1' },
    { id: 'gpt-5.1-codex', nome: 'GPT-5.1 Codex' },
    { id: 'gpt-5.1-codex-mini', nome: 'GPT-5.1 Codex mini (mais leve)' },
  ];
  const TOKENS_ENTRADA_REUNIAO = 15000;
  const TOKENS_SAIDA_REUNIAO = 1000;
  const dolar = (v) => `US$ ${v.toFixed(v < 0.1 ? 3 : 2).replace('.', ',')}`;

  function montarModelos(select, lista, descrever, atual) {
    select.replaceChildren(...lista.map((m) => new Option(m.nome, m.id)));
    if (atual && !lista.some((m) => m.id === atual)) select.append(new Option(`${atual} (personalizado)`, atual));
    select.value = atual || lista[0].id;
    const nota = $(`${select.id}-preco`);
    const mostrar = () => {
      const m = lista.find((x) => x.id === select.value);
      nota.textContent = m ? descrever(m) : 'Modelo personalizado: sem preço de referência aqui.';
    };
    select.onchange = mostrar;
    mostrar();
  }

  const descreverAnalise = (m) => {
    const media = (TOKENS_ENTRADA_REUNIAO * m.entrada + TOKENS_SAIDA_REUNIAO * m.saida) / 1e6;
    return `${dolar(m.entrada)} por 1 milhão de tokens lidos e ${dolar(m.saida)} por 1 milhão escritos. Média estimada: ${dolar(media)} por resumo de reunião de 1 hora.`;
  };
  const descreverChatGpt = () => 'Conta no limite da sua assinatura, sem custo em dólar. Se o modelo não estiver no seu plano, o ChatGPT recusa e o app mostra o erro: troque por outro.';
  const descreverTranscricao = (m) => `${dolar(m.minuto)} por minuto de áudio. Média estimada: ${dolar(m.minuto * 60)} por reunião de 1 hora.`;

  function preencherLista(select, nomes, atual) {
    select.replaceChildren(new Option('Padrão do sistema', ''));
    const todos = atual && !nomes.includes(atual) ? [...nomes, atual] : nomes;
    for (const n of todos) select.append(new Option(n === atual && !nomes.includes(n) ? `${n} (não encontrado)` : n, n));
    select.value = atual || '';
  }

  async function carregarDispositivos(c) {
    const d = await window.busca.listarDispositivos();
    preencherLista($('microfone'), d.microfones, c.microfone);
    preencherLista($('saida'), d.saidas, c.saida);
  }

  async function atualizarDrive(c) {
    const e = await window.busca.estadoDrive();
    const botao = $('drive-botao');
    $('drive-estado').textContent = e.conectado ? 'Conectado ao Google Drive.' : e.configurado ? 'Não conectado.' : (window.Ajustes.driveEmbutido ? 'Entre com a sua conta Google para anexar documentos.' : 'Preencha o ID e o segredo do cliente e salve.');
    botao.textContent = e.conectado ? 'Desconectar' : window.Ajustes.driveEmbutido ? 'Entrar com Google' : 'Conectar ao Google Drive';
    botao.disabled = !e.conectado && !e.configurado;
    window.Ajustes.pronto.drive = e.conectado;
  }

  /** Com ID e segredo embutidos no programa, o usuário só vê o botão de entrar. */
  function ajustarCamposDrive(embutido) {
    for (const id of ['driveClientId', 'driveClientSecret']) $(id).closest('label').hidden = embutido;
    $('drive-ajuda').hidden = embutido;
  }

  function atualizarChatGpt(c) {
    $('chatgpt-estado').className = 'estado';
    $('chatgpt-estado').textContent = c.chatgptConectado ? `Conectado${c.chatgptConta ? ` como ${c.chatgptConta}` : ''}.` : 'Não conectado.';
    $('chatgpt-botao').textContent = c.chatgptConectado ? 'Sair do ChatGPT' : 'Entrar com o ChatGPT';
    $('chatgpt-botao').disabled = false;
    $('chatgpt-modelo').hidden = $('chatgptModel-preco').hidden = !c.chatgptConectado;
    montarModelos($('chatgptModel'), MODELOS_CHATGPT, descreverChatGpt, c.chatgptModel);
  }

  /** Mostra só o que vale para o modo escolhido e diz em uma frase o que falta. */
  function mostrarModo(modo, c = {}) {
    $('bloco-openai').hidden = modo === 'local';
    $('bloco-locais').hidden = modo === 'openai';
    const resumo = $('modo-resumo');
    const falta = modo === 'openai' && !c.openaiKeyDefinida
      ? (c.chatgptConectado ? 'O ChatGPT cobre resumo, perguntas e tradução. Para transcrever o áudio na nuvem, falta a chave da API (ou escolha a transcrição no computador).' : 'Falta entrar com o ChatGPT ou informar a chave da API da OpenAI abaixo.')
      : modo === 'local' ? 'Baixe e escolha os modelos abaixo. Use "Testar" antes de uma reunião importante.' : '';
    resumo.className = 'estado';
    resumo.textContent = falta;
  }

  for (const r of document.querySelectorAll('input[name="modoIA"]')) {
    r.addEventListener('change', () => {
      window.ModelosLocais?.definirModo(r.value);
      mostrarModo(r.value, { openaiKeyDefinida: Boolean($('openaiKey').value || $('openaiKey').placeholder), chatgptConectado: window.Ajustes.chatgptConectado });
    });
  }

  async function carregar() {
    const c = await window.busca.lerConfig();
    window.Ajustes.driveEmbutido = Boolean(c.driveEmbutido);
    window.Ajustes.chatgptConectado = Boolean(c.chatgptConectado);
    atualizarChatGpt(c);
    ajustarCamposDrive(window.Ajustes.driveEmbutido);
    montarModelos($('openaiModel'), MODELOS_ANALISE, descreverAnalise, c.openaiModel);
    montarModelos($('transcricaoModel'), MODELOS_TRANSCRICAO, descreverTranscricao, c.transcricaoModel);
    for (const k of campos) {
      if (k === 'openaiModel' || k === 'transcricaoModel' || k === 'chatgptModel') continue;
      $(k).value = c[k] ?? '';
      if (segredos.includes(k)) $(k).placeholder = c[`${k}Definida`] ? 'Já salva. Deixe em branco para manter.' : '';
    }
    $('enviarAutomatico').checked = Boolean(c.enviarAutomatico);
    $('dicaTermos').checked = Boolean(c.dicaTermos);
    $('verificarAtualizacoes').checked = Boolean(c.verificarAtualizacoes);
    $('buscaSignificado').checked = Boolean(c.buscaSignificado);
    $('detectarReuniao').checked = Boolean(c.detectarReuniao);
    $('identificarFalantes').checked = Boolean(c.identificarFalantes);
    $('apagarAposDias').value = String(c.apagarAposDias ?? 0);
    window.Ajustes.pronto = { resumo: Boolean(c.openaiKeyDefinida || c.chatgptConectado || (c.transcricaoLocal && c.analiseLocal)), analiseLocal: Boolean(c.analiseLocal), email: Boolean(c.smtpUser && c.smtpPassDefinida && c.destinatario), drive: false };
    const modo = (await window.ModelosLocais?.carregar(c)) ?? 'openai';
    document.querySelector(`input[name="modoIA"][value="${modo}"]`).checked = true;
    mostrarModo(modo, c);
    await carregarDispositivos(c);
    await atualizarDrive(c);
  }

  async function salvar() {
    const dados = Object.fromEntries(campos.map((k) => [k, $(k).value.trim()]));
    dados.enviarAutomatico = $('enviarAutomatico').checked;
    dados.dicaTermos = $('dicaTermos').checked;
    dados.verificarAtualizacoes = $('verificarAtualizacoes').checked;
    dados.buscaSignificado = $('buscaSignificado').checked;
    dados.detectarReuniao = $('detectarReuniao').checked;
    dados.identificarFalantes = $('identificarFalantes').checked;
    dados.microfone = $('microfone').value;
    dados.saida = $('saida').value;
    Object.assign(dados, window.ModelosLocais?.valores());
    const dias = Number.parseInt($('apagarAposDias').value, 10);
    dados.apagarAposDias = Number.isFinite(dias) && dias > 0 ? dias : 0;
    const r = await window.busca.salvarConfig(dados);
    estado.className = r.ok ? 'estado' : 'erro';
    estado.textContent = r.ok ? 'Salvo.' : `Erro: ${r.erro}`;
    if (r.ok) await carregar();
    return r.ok;
  }

  // Links de ajuda abrem no navegador do sistema, sem tirar o app da tela.
  for (const a of document.querySelectorAll('a.link-externo')) {
    a.addEventListener('click', async (e) => {
      e.preventDefault();
      const r = await window.busca.abrirLink(a.href);
      if (!r.ok) estado.className = 'erro', estado.textContent = r.erro;
    });
  }

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    salvar();
  });

  $('chatgpt-botao').addEventListener('click', async () => {
    const botao = $('chatgpt-botao');
    botao.disabled = true;
    if (window.Ajustes.chatgptConectado) {
      await window.busca.desconectarChatGpt();
      return carregar();
    }
    $('chatgpt-estado').className = 'estado';
    $('chatgpt-estado').textContent = 'Siga o login no navegador que abriu...';
    const r = await window.busca.conectarChatGpt();
    await carregar();
    if (!r.ok) {
      $('chatgpt-estado').className = 'erro';
      $('chatgpt-estado').textContent = r.erro;
    }
  });

  $('drive-botao').addEventListener('click', async () => {
    const botao = $('drive-botao');
    const e = await window.busca.estadoDrive();
    if (e.conectado) {
      await window.busca.desconectarDrive();
      return atualizarDrive();
    }
    if (!(await salvar())) return;
    botao.disabled = true;
    $('drive-estado').className = 'estado';
    $('drive-estado').textContent = 'Siga o login no navegador que abriu...';
    const r = await window.busca.conectarDrive();
    await atualizarDrive();
    if (!r.ok) {
      $('drive-estado').className = 'erro';
      $('drive-estado').textContent = r.erro;
    }
  });

  // Atualizações: o app avisa ao abrir; a pessoa decide quando instalar (nunca no meio de uma reunião).
  const PAGINA_DE_VERSOES = 'https://github.com/rafaelamiranda/Pauta/releases/latest';
  const atualizacao = { versao: null, instalavel: false };

  function mostrarAtualizacao(r) {
    const botao = $('atualizacao-botao');
    const aviso = $('aviso-atualizacao');
    atualizacao.versao = r.disponivel ? r.versao : null;
    atualizacao.instalavel = Boolean(r.instalavel);
    $('atualizacao-estado').className = 'estado';
    aviso.hidden = !r.disponivel;
    if (!r.disponivel) {
      $('atualizacao-estado').textContent = `Você está na versão mais recente (${r.versaoAtual}).`;
      botao.textContent = 'Verificar agora';
      return;
    }
    aviso.textContent = `Atualizar para ${r.versao}`;
    $('atualizacao-estado').textContent = `Nova versão ${r.versao} disponível (você tem a ${r.versaoAtual}).`;
    botao.textContent = r.instalavel ? `Atualizar para ${r.versao}` : 'Baixar a nova versão';
  }

  $('atualizacao-botao').addEventListener('click', async () => {
    const botao = $('atualizacao-botao');
    const estadoAtual = $('atualizacao-estado');
    botao.disabled = true;
    try {
      if (atualizacao.versao && !atualizacao.instalavel) {
        const r = await window.busca.abrirLink(PAGINA_DE_VERSOES);
        if (!r.ok) estadoAtual.className = 'erro', estadoAtual.textContent = r.erro;
      } else if (atualizacao.versao) {
        estadoAtual.className = 'estado';
        estadoAtual.textContent = 'Baixando a atualização...';
        const r = await window.busca.instalarAtualizacao();
        if (!r.ok) estadoAtual.className = 'erro', estadoAtual.textContent = r.erro;
      } else {
        estadoAtual.className = 'estado';
        estadoAtual.textContent = 'Procurando...';
        const r = await window.busca.verificarAtualizacao();
        if (r.ok) mostrarAtualizacao(r);
        else estadoAtual.className = 'erro', estadoAtual.textContent = r.erro;
      }
    } finally {
      botao.disabled = false;
    }
  });
  // Vai para Ajustes (onde aparece o progresso) e faz o mesmo que o botão de lá.
  $('aviso-atualizacao').addEventListener('click', () => {
    document.querySelector('.aba[data-vista="ajustes"]').click();
    $('atualizacao-botao').click();
  });
  window.busca.aoAtualizacaoDisponivel(mostrarAtualizacao);
  window.busca.aoProgressoAtualizacao(({ baixado, total }) => {
    $('atualizacao-estado').textContent = total ? `Baixando a atualização... ${Math.min(100, Math.round((baixado / total) * 100))}%` : 'Baixando a atualização...';
  });

  const dadosEstado = $('dados-estado');
  const relatar = (r, ok) => {
    dadosEstado.className = r.ok ? 'estado' : 'erro';
    dadosEstado.textContent = r.ok ? ok(r) : `Erro: ${r.erro}`;
    if (r.ok) window.Conversas?.recarregar();
  };
  const plural = (n, um, varios) => `${n} ${n === 1 ? um : varios}`;

  $('backup').addEventListener('click', async () => {
    relatar(await window.busca.exportarBackup(), (r) => `Cópia salva em ${r.caminho}`);
  });
  confirmarDuplo($('apagar-antigas'), async () => {
    const dias = Number.parseInt($('apagarAposDias').value, 10);
    if (!(dias > 0)) {
      dadosEstado.className = 'erro';
      dadosEstado.textContent = 'Informe acima o número de dias (maior que zero).';
      return;
    }
    relatar(await window.busca.apagarConversasAntigas(dias), (r) => `${plural(r.apagadas, 'reunião apagada', 'reuniões apagadas')}.`);
  }, 'Confirmar: apagar reuniões antigas');
  confirmarDuplo($('apagar-tudo'), async () => {
    relatar(await window.busca.apagarTodasConversas(), (r) => `${plural(r.apagadas, 'conversa apagada', 'conversas apagadas')}.`);
  }, 'Confirmar: apagar tudo');

  window.Ajustes = { recarregar: carregar, pronto: { resumo: false, email: false, drive: false } };
  carregar();
})();
