'use strict';

// Liga a interface ao Rust (Tauri). Expõe o mesmo objeto window.busca que as outras telas usam.
(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const ouvir = (canal) => (fn) => { listen(canal, (e) => fn(e.payload)); };

  window.busca = {
    buscar: (q, limit, offset) => invoke('buscar', { q, limit, offset }),
    conversa: (id) => invoke('conversa', { id }),
    esconder: () => invoke('esconder'),
    modoMini: (mini) => invoke('modo_mini', { mini }),
    mostrarJanela: () => invoke('mostrar_janela'),
    sair: () => invoke('sair_app'),
    aoFocar: ouvir('foco-busca'),

    lerConfig: () => invoke('config_ler'),
    salvarConfig: (novos) => invoke('config_salvar', { novos }),

    listarLocais: () => invoke('locais_listar'),
    baixarLocal: (id) => invoke('local_baixar', { id }),
    cancelarLocal: (id) => invoke('local_cancelar', { id }),
    apagarLocal: (id) => invoke('local_apagar', { id }),
    testarLocal: (tipo) => invoke('local_testar', { tipo }),
    aoProgressoLocal: ouvir('local:progresso'),

    listarDispositivos: () => invoke('dispositivos_listar'),

    lerMaterial: () => invoke('material_ler'),
    salvarMaterial: (texto) => invoke('material_salvar', { texto }),
    listarDocumentos: () => invoke('documentos_listar'),
    removerDocumento: (id) => invoke('documento_remover', { id }),
    /** Envia o conteúdo do arquivo como corpo bruto; o nome vai codificado no cabeçalho. */
    anexarArquivo: (nome, bytes) => invoke('anexar_arquivo', bytes, { headers: { 'x-nome': encodeURIComponent(nome) } }),
    listarMateriais: () => invoke('materiais_listar'),
    guardarMaterial: (nome, id) => invoke('material_guardar', { nome, id: id ?? null }),
    carregarMaterial: (id) => invoke('material_carregar', { id }),
    excluirMaterial: (id) => invoke('material_excluir', { id }),

    abrirLink: (url) => invoke('abrir_link', { url }),
    estadoDrive: () => invoke('drive_estado'),
    conectarDrive: () => invoke('drive_conectar'),
    desconectarDrive: () => invoke('drive_desconectar'),
    buscarDrive: (q) => invoke('drive_buscar', { q }),
    anexarDoDrive: (id) => invoke('drive_anexar', { id }),

    perguntar: (pergunta, id) => invoke('perguntar', { pergunta, id: id ?? null }),

    iniciarReuniao: () => invoke('reuniao_iniciar'),
    pararReuniao: () => invoke('reuniao_parar'),
    reenviarEmail: (id) => invoke('reuniao_reenviar', { id }),
    listarPendencias: (incluirFeitas) => invoke('pendencias_listar', { incluirFeitas }),
    marcarPendencia: (sessao, indice, feita) => invoke('pendencia_marcar', { sessao, indice, feita }),

    apagarConversa: (id) => invoke('conversa_apagar', { id }),
    apagarTodasConversas: () => invoke('conversas_apagar_todas'),
    apagarConversasAntigas: (dias) => invoke('conversas_apagar_antigas', { dias }),
    textoDaConversa: (id) => invoke('exportar_texto', { id }),
    salvarConversaEmArquivo: (id) => invoke('exportar_arquivo', { id }),
    exportarBackup: () => invoke('backup_exportar'),

    aoAtalhoReuniao: ouvir('atalho:reuniao'),
    aoNivel: ouvir('reuniao:nivel'),
    aoEstado: ouvir('reuniao:estado'),
    aoFalar: ouvir('reuniao:fala'),
    aoAviso: ouvir('reuniao:aviso'),
    aoFinal: ouvir('reuniao:final'),
    aoEmail: ouvir('reuniao:email'),
  };
})();
