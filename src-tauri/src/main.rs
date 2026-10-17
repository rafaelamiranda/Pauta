#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod segredos;

use busca_core::chatgpt::ChatGpt;
use busca_core::drive::{ArquivoDrive, Drive};
use busca_core::exportar;
use busca_core::falantes::DiarizadorOpenAi;
use busca_core::mail::SmtpGmail;
use busca_core::locais::Locais;
use busca_core::material::{extrair_texto, Documento};
use busca_core::meeting::{Dependencias, Emitir, Reuniao};
use busca_core::roteador::{ResumidorRoteador, TranscritorRoteador};
use busca_core::settings::Configuracoes;
use busca_core::significado::Significado;
use busca_core::traducao::{DependenciasTraducao, Entrada, Traducao, IDIOMAS};
use busca_core::store::{Busca, OpcoesBusca, PendenciaGeral, Store};
use busca_core::transcricao::TranscricaoOpenAi;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::{InvokeBody, Request};
use tauri::{Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const USUARIO: &str = "local";
const PAGINA_DE_VERSOES: &str = "https://github.com/rafaelamiranda/Pauta/releases/latest";
// Tamanhos lógicos da janela: cheia (igual ao tauri.conf.json) e o mini player.
const LARGURA: f64 = 760.0;
const ALTURA: f64 = 620.0;
const MINI_LARGURA: f64 = 420.0;
const MINI_ALTURA: f64 = 96.0;

struct Estado {
    store: Arc<Mutex<Store>>,
    config: Arc<Configuracoes>,
    reuniao: Arc<Reuniao>,
    drive: Arc<Drive>,
    chatgpt: Arc<ChatGpt>,
    significado: Arc<Significado>,
    /// Versão nova achada na última verificação, à espera de o usuário mandar instalar.
    atualizacao: Mutex<Option<tauri_plugin_updater::Update>>,
    locais: Arc<Locais>,
    captura: Mutex<Option<audio::Captura>>,
    traducao: Arc<Traducao>,
    captura_traducao: Mutex<Option<audio::Captura>>,
    arquivo_material: PathBuf,
    arquivo_documentos: PathBuf,
}

fn erro(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Resposta padrão dos comandos: `{ok: true, ...}` ou `{ok: false, erro}`.
fn resposta(r: Result<Value, String>) -> Value {
    match r {
        Ok(Value::Object(mut m)) => {
            m.insert("ok".into(), true.into());
            Value::Object(m)
        }
        Ok(_) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "erro": e }),
    }
}

impl Estado {
    /// Há uma reunião ou uma tradução usando a captura de áudio.
    fn capturando(&self) -> Result<bool, String> {
        Ok(self.captura.lock().map_err(erro)?.is_some() || self.captura_traducao.lock().map_err(erro)?.is_some())
    }

    /// Guarda em arquivo o texto colado e os documentos, para sobreviverem ao fechamento do app.
    fn persistir_material(&self) -> Result<(), String> {
        std::fs::write(&self.arquivo_material, self.reuniao.texto_material()).map_err(erro)?;
        std::fs::write(&self.arquivo_documentos, serde_json::to_string(&self.reuniao.documentos()).map_err(erro)?).map_err(erro)
    }

    fn documentos_para_tela(&self) -> Value {
        Value::Array(
            self.reuniao
                .documentos()
                .iter()
                .map(|d| json!({ "id": d.id, "nome": d.nome, "origem": d.origem, "caracteres": d.texto.chars().count() }))
                .collect(),
        )
    }
}

#[tauri::command]
async fn buscar(estado: State<'_, Estado>, q: Option<String>, limit: Option<i64>, offset: Option<i64>) -> Result<Busca, String> {
    // Busca por significado: indexa o que falta e vetoriza o texto digitado. Se falhar (sem internet,
    // sem chave), a busca de texto segue sozinha.
    let mut vetor = None;
    if let Some(texto) = q.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        if !estado.config.chave_significado().is_empty() {
            let _ = estado.significado.indexar_pendentes(&estado.store, USUARIO).await;
            vetor = estado.significado.vetor(texto).await.ok();
        }
    }
    let store = estado.store.lock().map_err(erro)?;
    store
        .buscar(USUARIO, OpcoesBusca { texto: q.as_deref(), limit: limit.unwrap_or(20), offset: offset.unwrap_or(0), excluir_ativas: true, significado: vetor.as_deref() })
        .map_err(erro)
}

#[tauri::command]
async fn conversa(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    let store = estado.store.lock().map_err(erro)?;
    let sessao = store.obter_sessao(&id).map_err(erro)?;
    let falas = store.transcricao(&id).map_err(erro)?;
    Ok(json!({ "sessao": sessao, "falas": falas }))
}

#[tauri::command]
fn esconder(janela: tauri::WebviewWindow) {
    let _ = janela.hide();
}

/// Alterna entre a janela cheia e o mini player, mantendo o centro horizontal no lugar.
#[tauri::command]
fn modo_mini(janela: tauri::WebviewWindow, mini: bool) -> Result<(), String> {
    let (largura, altura) = if mini { (MINI_LARGURA, MINI_ALTURA) } else { (LARGURA, ALTURA) };
    let escala = janela.scale_factor().map_err(erro)?;
    let pos = janela.outer_position().map_err(erro)?;
    let atual = janela.outer_size().map_err(erro)?;
    let nova_largura = (largura * escala).round() as i32;
    let x = pos.x + (atual.width as i32 - nova_largura) / 2;
    // No Linux (GTK), set_size sozinho é ignorado quando a janela não é redimensionável (ou tem tamanho mínimo
    // do conteúdo): fixa mínimo e máximo no tamanho novo, o que força o gerenciador de janelas a obedecer.
    #[cfg(target_os = "linux")]
    {
        let novo = tauri::LogicalSize::new(largura, altura);
        janela.set_resizable(true).map_err(erro)?;
        janela.set_min_size(Some(novo)).map_err(erro)?;
        janela.set_max_size(Some(novo)).map_err(erro)?;
    }
    janela.set_size(tauri::LogicalSize::new(largura, altura)).map_err(erro)?;
    janela.set_position(tauri::PhysicalPosition::new(x, pos.y)).map_err(erro)
}

#[tauri::command]
fn mostrar_janela(janela: tauri::WebviewWindow) {
    mostrar(&janela);
}

#[tauri::command]
async fn config_ler(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(estado.config.para_tela())
}

#[tauri::command]
async fn config_salvar(estado: State<'_, Estado>, novos: Value) -> Result<Value, String> {
    let r = estado.config.salvar(&novos).map(|()| json!({})).map_err(erro);
    if r.is_ok() && !estado.capturando()? {
        // fora de reunião, desliga os servidores locais que deixaram de ser usados
        estado.locais.liberar_nao_usados(&estado.config.obter()).await;
    }
    Ok(resposta(r))
}

// ---- modelos locais (opcionais) ----

#[tauri::command]
async fn locais_listar(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(estado.locais.listar(&estado.config.obter().aceleracao))
}

#[tauri::command]
async fn local_baixar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    Ok(resposta(estado.locais.iniciar_download(&id, &estado.config.obter().aceleracao).map(|()| json!({})).map_err(erro)))
}

#[tauri::command]
async fn local_cancelar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    estado.locais.cancelar(&id);
    Ok(json!({ "ok": true }))
}

#[tauri::command]
async fn local_apagar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    if estado.capturando()? {
        return Ok(json!({ "ok": false, "erro": "Finalize a reunião ou a tradução em andamento antes de apagar um modelo." }));
    }
    Ok(resposta(estado.locais.apagar(&id).await.map(|()| json!({})).map_err(erro)))
}

/// Liga o servidor local e confere que ele responde. `tipo`: "whisper" ou "llm".
#[tauri::command]
async fn local_testar(estado: State<'_, Estado>, tipo: String) -> Result<Value, String> {
    let cfg = estado.config.obter();
    let r = if tipo == "whisper" { estado.locais.testar_whisper(&cfg).await } else { estado.locais.testar_llm(&cfg).await };
    Ok(resposta(r.map(|s| json!({ "segundos": s })).map_err(erro)))
}

#[tauri::command]
async fn dispositivos_listar() -> Result<Value, String> {
    let (microfones, saidas) = audio::dispositivos();
    Ok(json!({ "microfones": microfones, "saidas": saidas }))
}

// ---- material de apoio ----

#[tauri::command]
async fn material_ler(estado: State<'_, Estado>) -> Result<String, String> {
    Ok(estado.reuniao.texto_material())
}

/// Guarda o texto colado do material de apoio.
#[tauri::command]
async fn material_salvar(estado: State<'_, Estado>, texto: String) -> Result<Value, String> {
    Ok(resposta(estado.reuniao.definir_material(&texto).map_err(erro).and_then(|()| estado.persistir_material()).map(|()| json!({}))))
}

#[tauri::command]
async fn documentos_listar(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(estado.documentos_para_tela())
}

#[tauri::command]
async fn documento_remover(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    Ok(resposta(
        estado.reuniao.remover_documento(&id).map_err(erro).and_then(|()| estado.persistir_material()).map(|()| json!({ "documentos": estado.documentos_para_tela() })),
    ))
}

/// Anexa um arquivo do computador. O conteúdo vai no corpo do pedido e o nome (codificado) no cabeçalho `x-nome`.
#[tauri::command]
async fn anexar_arquivo(estado: State<'_, Estado>, pedido: Request<'_>) -> Result<Value, String> {
    let nome = pedido
        .headers()
        .get("x-nome")
        .and_then(|v| v.to_str().ok())
        .map(decodificar_nome)
        .unwrap_or_else(|| "arquivo".into());
    let InvokeBody::Raw(bytes) = pedido.body() else { return Ok(json!({ "ok": false, "erro": "O arquivo não chegou." })) };
    let bytes = bytes.clone();
    let nome_leitura = nome.clone();
    let texto = tauri::async_runtime::spawn_blocking(move || extrair_texto(&nome_leitura, "", &bytes)).await.map_err(erro)?;
    Ok(resposta(texto.map_err(erro).and_then(|texto| {
        let doc = Documento { id: format!("arquivo:{}", uuid_curto()), nome, origem: "arquivo".into(), texto };
        estado.reuniao.anexar_documento(doc).map_err(erro)?;
        estado.persistir_material()?;
        Ok(json!({ "documentos": estado.documentos_para_tela() }))
    })))
}

fn uuid_curto() -> String {
    format!("{:x}", busca_core::agora_ms())
}

/// Decodifica `%XX` do nome do arquivo enviado pela tela.
fn decodificar_nome(v: &str) -> String {
    let b = v.as_bytes();
    let mut saida = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(n) = v.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                saida.push(n);
                i += 3;
                continue;
            }
        }
        saida.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&saida).into_owned()
}

// ---- materiais salvos (cliente ou projeto) ----

#[tauri::command]
async fn materiais_listar(estado: State<'_, Estado>) -> Result<Value, String> {
    let lista = estado.store.lock().map_err(erro)?.listar_materiais(USUARIO).map_err(erro)?;
    Ok(Value::Array(
        lista
            .iter()
            .map(|m| json!({ "id": m.id, "nome": m.nome, "caracteres": m.texto.chars().count(), "documentos": m.documentos.len() }))
            .collect(),
    ))
}

/// Salva o material atual (texto e documentos) com um nome. Com `id`, atualiza o material existente.
#[tauri::command]
async fn material_guardar(estado: State<'_, Estado>, nome: String, id: Option<String>) -> Result<Value, String> {
    Ok(resposta(
        estado
            .store
            .lock()
            .map_err(erro)?
            .salvar_material(USUARIO, id.as_deref(), &nome, &estado.reuniao.texto_material(), &estado.reuniao.documentos(), busca_core::agora_ms() / 1000)
            .map(|id| json!({ "id": id }))
            .map_err(erro),
    ))
}

/// Troca o material atual pelo material salvo.
#[tauri::command]
async fn material_carregar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    let achado = estado.store.lock().map_err(erro)?.listar_materiais(USUARIO).map_err(erro)?.into_iter().find(|m| m.id == id);
    let Some(m) = achado else { return Ok(json!({ "ok": false, "erro": "Material não encontrado." })) };
    Ok(resposta(
        estado
            .reuniao
            .definir_material(&m.texto)
            .and_then(|()| estado.reuniao.definir_documentos(m.documentos.clone()))
            .map_err(erro)
            .and_then(|()| estado.persistir_material())
            .map(|()| json!({ "texto": m.texto, "documentos": estado.documentos_para_tela() })),
    ))
}

#[tauri::command]
async fn material_excluir(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    Ok(resposta(estado.store.lock().map_err(erro)?.apagar_material(USUARIO, &id).map(|()| json!({})).map_err(erro)))
}

// ---- Google Drive ----

#[tauri::command]
async fn drive_estado(estado: State<'_, Estado>) -> Result<Value, String> {
    let c = estado.config.obter();
    Ok(json!({
        "conectado": estado.drive.conectado(),
        "configurado": !c.drive_client_id.is_empty() && !c.drive_client_secret.is_empty(),
    }))
}

fn abrir_no_navegador(url: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = url;
        Err(anyhow::anyhow!("Abrir o navegador só é suportado no Windows e no Linux."))
    }
}

/// Abre no navegador só os endereços de ajuda conhecidos (a tela não escolhe um endereço qualquer).
#[tauri::command]
fn abrir_link(url: String) -> Value {
    const PERMITIDOS: [&str; 3] = ["https://platform.openai.com/api-keys", "https://myaccount.google.com/apppasswords", PAGINA_DE_VERSOES];
    if !PERMITIDOS.contains(&url.as_str()) {
        return resposta(Err("Endereço não permitido.".to_string()));
    }
    resposta(abrir_no_navegador(&url).map(|()| json!({})).map_err(erro))
}

/// A atualização se instala sozinha no Windows e no AppImage. Em .deb/.rpm ou no executável solto, a pessoa baixa a nova versão.
fn atualizacao_instalavel() -> bool {
    cfg!(windows) || (cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some())
}

/// Procura versão nova no GitHub e guarda a atualização para instalar depois.
async fn verificar_atualizacao(app: &tauri::AppHandle) -> Result<Value, String> {
    let atual = app.package_info().version.to_string();
    let update = app.updater().map_err(erro)?.check().await.map_err(|e| format!("Não foi possível verificar atualizações: {e}"))?;
    let resultado = match &update {
        Some(u) => json!({ "disponivel": true, "versaoAtual": atual, "versao": u.version, "notas": u.body, "instalavel": atualizacao_instalavel() }),
        None => json!({ "disponivel": false, "versaoAtual": atual }),
    };
    *app.state::<Estado>().atualizacao.lock().map_err(erro)? = update;
    Ok(resultado)
}

#[tauri::command]
async fn atualizacao_verificar(app: tauri::AppHandle) -> Result<Value, String> {
    Ok(resposta(verificar_atualizacao(&app).await))
}

/// Baixa a versão achada na verificação, instala e reabre o app.
#[tauri::command]
async fn atualizacao_instalar(app: tauri::AppHandle, estado: State<'_, Estado>) -> Result<Value, String> {
    if estado.capturando()? {
        return Ok(json!({ "ok": false, "erro": "Finalize a reunião ou a tradução em andamento antes de atualizar." }));
    }
    if !atualizacao_instalavel() {
        return Ok(json!({ "ok": false, "erro": "Esta instalação não se atualiza sozinha. Baixe a nova versão na página de versões." }));
    }
    let Some(update) = estado.atualizacao.lock().map_err(erro)?.take() else {
        return Ok(json!({ "ok": false, "erro": "Verifique as atualizações primeiro." }));
    };
    let (h, mut baixado) = (app.clone(), 0u64);
    let r = update
        .download_and_install(
            move |pedaco, total| {
                baixado += pedaco as u64;
                let _ = h.emit("atualizacao:progresso", json!({ "baixado": baixado, "total": total }));
            },
            || {},
        )
        .await;
    match r {
        Ok(()) => {
            estado.locais.parar_tudo();
            app.restart()
        }
        Err(e) => Ok(json!({ "ok": false, "erro": format!("Não foi possível atualizar: {e}") })),
    }
}

/// Entrar com a conta do ChatGPT: abre o login no navegador e guarda a permissão.
#[tauri::command]
async fn chatgpt_conectar(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(resposta(estado.chatgpt.conectar(abrir_no_navegador).await.map(|()| json!({})).map_err(erro)))
}

#[tauri::command]
async fn chatgpt_desconectar(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(resposta(estado.chatgpt.desconectar().await.map(|()| json!({})).map_err(erro)))
}

#[tauri::command]
async fn drive_conectar(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(resposta(estado.drive.conectar(abrir_no_navegador).await.map(|()| json!({})).map_err(erro)))
}

#[tauri::command]
async fn drive_desconectar(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(resposta(estado.drive.desconectar().map(|()| json!({})).map_err(erro)))
}

#[tauri::command]
async fn drive_buscar(estado: State<'_, Estado>, q: String) -> Result<Value, String> {
    Ok(resposta(estado.drive.buscar(&q).await.map(|a: Vec<ArquivoDrive>| json!({ "arquivos": a })).map_err(erro)))
}

/// Baixa o arquivo do Drive e o anexa ao material de apoio.
#[tauri::command]
async fn drive_anexar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    let doc = match estado.drive.baixar(&id).await {
        Ok(d) => d,
        Err(e) => return Ok(json!({ "ok": false, "erro": e.to_string() })),
    };
    Ok(resposta(
        estado.reuniao.anexar_documento(doc).map_err(erro).and_then(|()| estado.persistir_material()).map(|()| json!({ "documentos": estado.documentos_para_tela() })),
    ))
}

// ---- perguntas ----

#[tauri::command]
async fn perguntar(app: tauri::AppHandle, estado: State<'_, Estado>, pergunta: String, id: Option<String>, pedido: Option<String>) -> Result<Value, String> {
    // com `pedido`, a resposta vai aparecendo na tela (evento pergunta:parcial) enquanto é escrita
    let parcial = move |texto: &str| {
        if let Some(p) = &pedido {
            let _ = app.emit("pergunta:parcial", json!({ "pedido": p, "texto": texto }));
        }
    };
    Ok(match estado.reuniao.perguntar_parcial(&pergunta, id.as_deref(), &parcial).await {
        Ok(resposta) => json!({ "ok": true, "resposta": resposta }),
        Err(e) => json!({ "ok": false, "erro": e.to_string() }),
    })
}

// ---- reunião ----

#[tauri::command]
async fn reuniao_iniciar(estado: State<'_, Estado>) -> Result<Value, String> {
    let cfg = estado.config.obter();
    if estado.captura_traducao.lock().map_err(erro)?.is_some() {
        return Ok(json!({ "ok": false, "erro": "Pare a tradução em andamento antes de iniciar uma reunião." }));
    }
    if cfg.openai_key.is_empty() && !(cfg.transcricao_local && (cfg.analise_local || cfg.chatgpt_conectado)) {
        return Ok(json!({ "ok": false, "erro": "Configure a chave da OpenAI na aba Ajustes, ou escolha a transcrição local junto com a análise local ou o login do ChatGPT. A chave é usada para o que não for local." }));
    }
    if cfg.transcricao_local {
        // sobe o servidor de voz antes de começar, para a primeira fala não esperar o modelo carregar
        if let Err(e) = estado.locais.garantir_whisper(&cfg).await {
            return Ok(json!({ "ok": false, "erro": format!("{e} Em Ajustes, escolha a OpenAI para usar a nuvem.") }));
        }
    }
    if let Err(e) = estado.reuniao.iniciar() {
        return Ok(json!({ "ok": false, "erro": e.to_string() }));
    }
    match audio::iniciar(estado.reuniao.clone(), cfg.microfone, cfg.saida) {
        Ok((captura, avisos)) => {
            *estado.captura.lock().map_err(erro)? = Some(captura);
            Ok(json!({ "ok": true, "avisos": avisos }))
        }
        Err(e) => {
            estado.reuniao.descartar();
            Ok(json!({ "ok": false, "erro": e }))
        }
    }
}

#[tauri::command]
async fn reuniao_parar(estado: State<'_, Estado>) -> Result<Value, String> {
    // Fechar a captura primeiro: encerra as threads de áudio antes do resumo.
    let captura = estado.captura.lock().map_err(erro)?.take();
    drop(captura);
    Ok(match estado.reuniao.parar().await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "erro": e.to_string() }),
    })
}

#[tauri::command]
async fn reuniao_reenviar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    estado.reuniao.enviar_por_email(&id, None).await;
    Ok(json!({ "ok": true }))
}

// ---- tradução em tempo real ----

#[tauri::command]
fn traducao_idiomas() -> Value {
    Value::Array(IDIOMAS.iter().map(|(codigo, nome)| json!({ "codigo": codigo, "nome": nome })).collect())
}

#[tauri::command]
async fn traducao_iniciar(estado: State<'_, Estado>, origem: String, destino: String, entrada: String) -> Result<Value, String> {
    let cfg = estado.config.obter();
    if estado.capturando()? {
        return Ok(json!({ "ok": false, "erro": "Finalize a reunião em andamento antes de traduzir." }));
    }
    if cfg.openai_key.is_empty() && !(cfg.transcricao_local && (cfg.analise_local || cfg.chatgpt_conectado)) {
        return Ok(json!({ "ok": false, "erro": "Configure a chave da OpenAI na aba Ajustes, ou escolha a transcrição local junto com a análise local ou o login do ChatGPT. A chave é usada para o que não for local." }));
    }
    // sobe os servidores locais antes, para a primeira fala não esperar o modelo carregar
    if cfg.transcricao_local {
        if let Err(e) = estado.locais.garantir_whisper(&cfg).await {
            return Ok(json!({ "ok": false, "erro": format!("{e} Em Ajustes, escolha a OpenAI para usar a nuvem.") }));
        }
    }
    if cfg.analise_local {
        if let Err(e) = estado.locais.garantir_llm(&cfg).await {
            return Ok(json!({ "ok": false, "erro": format!("{e} Em Ajustes, escolha a OpenAI para usar a nuvem.") }));
        }
    }
    if let Err(e) = estado.traducao.iniciar(&origem, &destino, Entrada::de_texto(&entrada)) {
        return Ok(json!({ "ok": false, "erro": e.to_string() }));
    }
    match audio::iniciar(estado.traducao.clone(), cfg.microfone, cfg.saida) {
        Ok((captura, avisos)) => {
            *estado.captura_traducao.lock().map_err(erro)? = Some(captura);
            Ok(json!({ "ok": true, "avisos": avisos }))
        }
        Err(e) => {
            let _ = estado.traducao.parar().await;
            Ok(json!({ "ok": false, "erro": e }))
        }
    }
}

#[tauri::command]
async fn traducao_parar(estado: State<'_, Estado>, enviar: bool) -> Result<Value, String> {
    // fechar a captura primeiro: encerra as threads de áudio antes de traduzir o que sobrou
    let captura = estado.captura_traducao.lock().map_err(erro)?.take();
    drop(captura);
    Ok(match estado.traducao.parar().await {
        Ok(()) => {
            if enviar && estado.traducao.tem_texto() {
                // o envio segue em segundo plano; a tela acompanha pelo evento traducao:email
                let t = estado.traducao.clone();
                tauri::async_runtime::spawn(async move { t.enviar_por_email().await });
            }
            json!({ "ok": true })
        }
        Err(e) => json!({ "ok": false, "erro": e.to_string() }),
    })
}

#[tauri::command]
async fn traducao_enviar_email(estado: State<'_, Estado>) -> Result<Value, String> {
    if !estado.traducao.tem_texto() {
        return Ok(json!({ "ok": false, "erro": "Ainda não há nada traduzido para enviar." }));
    }
    let t = estado.traducao.clone();
    tauri::async_runtime::spawn(async move { t.enviar_por_email().await });
    Ok(json!({ "ok": true }))
}

#[tauri::command]
fn traducao_limpar(estado: State<'_, Estado>) {
    estado.traducao.limpar();
}

// ---- pendências ----

#[tauri::command]
async fn pendencias_listar(estado: State<'_, Estado>, incluir_feitas: bool) -> Result<Vec<PendenciaGeral>, String> {
    estado.store.lock().map_err(erro)?.listar_pendencias(USUARIO, incluir_feitas).map_err(erro)
}

#[tauri::command]
async fn pendencia_marcar(estado: State<'_, Estado>, sessao: String, indice: usize, feita: bool) -> Result<Value, String> {
    Ok(resposta(estado.store.lock().map_err(erro)?.marcar_pendencia(&sessao, indice, feita).map(|()| json!({})).map_err(erro)))
}

// ---- apagar, exportar e backup ----

#[tauri::command]
async fn conversa_apagar(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    let store = estado.store.lock().map_err(erro)?;
    let r = match store.obter_sessao(&id).map_err(erro)? {
        Some(s) if s.status == "active" => Err("A reunião ainda está em andamento.".to_string()),
        Some(_) => store.apagar_sessao(&id).map(|()| json!({})).map_err(erro),
        None => Err("Conversa não encontrada.".to_string()),
    };
    Ok(resposta(r))
}

#[tauri::command]
async fn conversas_apagar_todas(estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(resposta(estado.store.lock().map_err(erro)?.apagar_todas(USUARIO).map(|n| json!({ "apagadas": n })).map_err(erro)))
}

#[tauri::command]
async fn conversas_apagar_antigas(estado: State<'_, Estado>, dias: i64) -> Result<Value, String> {
    if dias < 1 {
        return Ok(json!({ "ok": false, "erro": "Informe um número de dias maior que zero." }));
    }
    let limite = busca_core::agora_ms() / 1000 - dias * 86_400;
    Ok(resposta(estado.store.lock().map_err(erro)?.apagar_antes_de(USUARIO, limite).map(|n| json!({ "apagadas": n })).map_err(erro)))
}

fn markdown_da_conversa(estado: &Estado, id: &str) -> Result<(String, String), String> {
    let store = estado.store.lock().map_err(erro)?;
    let sessao = store.obter_sessao(id).map_err(erro)?.ok_or("Conversa não encontrada.")?;
    let falas = store.transcricao(id).map_err(erro)?;
    Ok((exportar::markdown(&sessao, &falas), sessao.subject.unwrap_or_else(|| "Reunião".into())))
}

/// Devolve um caminho que ainda não existe: `nome.ext`, `nome (2).ext`, ...
fn caminho_livre(pasta: &Path, nome: &str) -> PathBuf {
    let (base, ext) = nome.rsplit_once('.').map(|(b, e)| (b.to_string(), format!(".{e}"))).unwrap_or((nome.to_string(), String::new()));
    let mut c = pasta.join(nome);
    let mut n = 2;
    while c.exists() {
        c = pasta.join(format!("{base} ({n}){ext}"));
        n += 1;
    }
    c
}

#[tauri::command]
async fn exportar_texto(estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    Ok(resposta(markdown_da_conversa(&estado, &id).map(|(texto, _)| json!({ "texto": texto }))))
}

/// Salva a conversa em Markdown na pasta Downloads.
#[tauri::command]
async fn exportar_arquivo(app: tauri::AppHandle, estado: State<'_, Estado>, id: String) -> Result<Value, String> {
    Ok(resposta((|| {
        let (texto, titulo) = markdown_da_conversa(&estado, &id)?;
        let pasta = app.path().download_dir().map_err(erro)?;
        let caminho = caminho_livre(&pasta, &exportar::nome_de_arquivo(&titulo, "md"));
        std::fs::write(&caminho, texto).map_err(erro)?;
        Ok(json!({ "caminho": caminho.to_string_lossy() }))
    })()))
}

/// Copia o banco de conversas para a pasta Downloads.
#[tauri::command]
async fn backup_exportar(app: tauri::AppHandle, estado: State<'_, Estado>) -> Result<Value, String> {
    Ok(resposta((|| {
        let pasta = app.path().download_dir().map_err(erro)?;
        let caminho = caminho_livre(&pasta, "conversas-backup.db");
        estado.store.lock().map_err(erro)?.copiar_para(&caminho).map_err(erro)?;
        Ok(json!({ "caminho": caminho.to_string_lossy() }))
    })()))
}

// ---- janela, bandeja e atalhos ----

/// Fecha o app, a não ser que haja reunião em andamento (o resumo e o e-mail se perderiam).
fn tentar_sair(app: &tauri::AppHandle) -> Result<(), String> {
    let gravando = app.state::<Estado>().captura.lock().map_err(erro)?.is_some();
    if gravando {
        if let Some(j) = app.get_webview_window("main") {
            mostrar(&j);
        }
        let aviso = "Há uma reunião em andamento. Finalize a reunião antes de fechar o app.";
        let _ = app.emit("reuniao:aviso", aviso);
        return Err(aviso.into());
    }
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn sair_app(app: tauri::AppHandle) -> Value {
    resposta(tentar_sair(&app).map(|()| json!({})))
}

/// Ícone na bandeja: clique alterna a janela; o menu tem Mostrar/Esconder e Sair.
fn criar_bandeja(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let alternar_item = MenuItem::with_id(app, "alternar", "Mostrar ou esconder (Ctrl+Shift+Espaço)", true, None::<&str>)?;
    let sair_item = MenuItem::with_id(app, "sair", "Sair da Pauta", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&alternar_item, &sair_item])?;
    let mut b = TrayIconBuilder::new()
        .tooltip("Pauta")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "alternar" => alternar(app),
            "sair" => {
                let _ = tentar_sair(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                alternar(tray.app_handle());
            }
        });
    if let Some(icone) = app.default_window_icon() {
        b = b.icon(icone.clone());
    }
    b.build(app)?;
    Ok(())
}

fn alternar(app: &tauri::AppHandle) {
    let Some(j) = app.get_webview_window("main") else { return };
    if j.is_visible().unwrap_or(false) {
        let _ = j.hide();
    } else {
        mostrar(&j);
    }
}

fn mostrar(j: &tauri::WebviewWindow) {
    let _ = j.show();
    let _ = j.set_focus();
    let _ = j.emit("foco-busca", ());
}

/// Centraliza a janela no topo do monitor principal.
fn posicionar(j: &tauri::WebviewWindow) {
    if let (Ok(Some(m)), Ok(tam)) = (j.primary_monitor(), j.outer_size()) {
        let x = m.position().x + (m.size().width as i32 - tam.width as i32) / 2;
        let y = m.position().y + (60.0 * m.scale_factor()) as i32;
        let _ = j.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let dados = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dados)?;

            let store = Arc::new(Mutex::new(Store::abrir(&dados.join("conversas.db"))?));
            // ID e segredo do Google embutidos na hora de gerar o instalador (veja o .env.example)
            let config = Arc::new(
                Configuracoes::new(dados.join("config.json"), Arc::new(segredos::Cofre::default()))
                    .com_drive_embutido(option_env!("GOOGLE_CLIENT_ID").unwrap_or(""), option_env!("GOOGLE_CLIENT_SECRET").unwrap_or("")),
            );

            // apagar sozinho as reuniões antigas, se a pessoa pediu
            let dias = config.obter().apagar_apos_dias;
            if dias > 0 {
                let limite = busca_core::agora_ms() / 1000 - i64::from(dias) * 86_400;
                if let Err(e) = store.lock().unwrap().apagar_antes_de(USUARIO, limite) {
                    eprintln!("Não foi possível apagar reuniões antigas: {e}");
                }
            }

            let app_h = app.handle().clone();
            let emitir: Emitir = Arc::new(move |canal, dados| {
                let _ = app_h.emit(canal, dados);
            });
            let locais = Locais::new(dados.join("locais"), emitir.clone());
            let cfg = config.clone();
            let ler_config: Arc<dyn Fn() -> busca_core::settings::Config + Send + Sync> = Arc::new(move || cfg.obter());
            let cfg_transc = config.clone();
            let cfg_idioma = config.clone();
            let cfg_drive = config.clone();
            let nuvem = TranscricaoOpenAi::new(Arc::new(move || {
                let c = cfg_transc.obter();
                (c.openai_key, c.transcricao_model)
            }))
            .com_idioma(Arc::new(move || cfg_idioma.obter().idioma));
            let transcritor = Arc::new(TranscritorRoteador::new(nuvem, locais.clone(), ler_config.clone()));
            let chatgpt = Arc::new(ChatGpt::new(config.segredos()));
            let resumidor = Arc::new(ResumidorRoteador::new(locais.clone(), ler_config.clone(), chatgpt.clone()));
            let traducao = Arc::new(Traducao::new(DependenciasTraducao {
                transcritor: transcritor.clone(),
                resumidor: resumidor.clone(),
                config: ler_config.clone(),
                emitir: emitir.clone(),
                agora: Arc::new(busca_core::agora_ms),
                remetente: Arc::new(SmtpGmail),
            }));
            let cfg_chave = config.clone();
            let reuniao = Arc::new(Reuniao::new(Dependencias {
                store: store.clone(),
                usuario: USUARIO.into(),
                transcritor,
                resumidor,
                remetente: Arc::new(SmtpGmail),
                config: ler_config,
                emitir,
                agora: Arc::new(busca_core::agora_ms),
                diarizador: Some(Arc::new(DiarizadorOpenAi::new(Arc::new(move || cfg_chave.obter().openai_key)))),
            }));
            let cfg_significado = config.clone();
            let significado = Arc::new(Significado::new(Arc::new(move || cfg_significado.chave_significado())));
            let drive = Arc::new(Drive::new(
                config.segredos(),
                Arc::new(move || {
                    let c = cfg_drive.obter();
                    (c.drive_client_id, c.drive_client_secret)
                }),
            ));

            let arquivo_material = dados.join("material.txt");
            let arquivo_documentos = dados.join("documentos.json");
            if let Ok(texto) = std::fs::read_to_string(&arquivo_material) {
                let _ = reuniao.definir_material(&texto);
            }
            if let Some(docs) = std::fs::read_to_string(&arquivo_documentos).ok().and_then(|t| serde_json::from_str::<Vec<Documento>>(&t).ok()) {
                let _ = reuniao.definir_documentos(docs);
            }
            let verificar_ao_abrir = config.obter().verificar_atualizacoes;
            // indexa em segundo plano as conversas que ainda não têm trechos para a busca por significado
            if !config.chave_significado().is_empty() {
                let (store, significado) = (store.clone(), significado.clone());
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = significado.indexar_pendentes(&store, USUARIO).await {
                        eprintln!("Busca por significado: {e}");
                    }
                });
            }
            app.manage(Estado { store, config, reuniao, drive, chatgpt, significado, atualizacao: Mutex::new(None), locais, captura: Mutex::new(None), traducao, captura_traducao: Mutex::new(None), arquivo_material, arquivo_documentos });

            if verificar_ao_abrir {
                // espera o app terminar de abrir; se houver versão nova, a tela mostra um aviso
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                    match verificar_atualizacao(&h).await {
                        Ok(v) if v["disponivel"] == true => {
                            let _ = h.emit("atualizacao:disponivel", v);
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("{e}"),
                    }
                });
            }

            let atalho = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            let atalho_reuniao = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyR);
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |app, sc, ev| {
                        if ev.state() != ShortcutState::Pressed {
                            return;
                        }
                        if sc == &atalho {
                            alternar(app);
                        } else if sc == &atalho_reuniao {
                            let _ = app.emit("atalho:reuniao", ());
                        }
                    })
                    .build(),
            )?;
            if let Err(e) = app.global_shortcut().register(atalho) {
                eprintln!("Não foi possível registrar o atalho Ctrl+Shift+Espaço: {e}");
            }
            if let Err(e) = app.global_shortcut().register(atalho_reuniao) {
                eprintln!("Não foi possível registrar o atalho Ctrl+Shift+R: {e}");
            }

            // No Wayland o sistema não deixa o app registrar atalhos globais. Alternativa: um atalho do próprio
            // sistema que manda um sinal ao app (`pkill -USR1 -x pauta` mostra/esconde, `-USR2` inicia/finaliza a reunião).
            #[cfg(target_os = "linux")]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let (Ok(mut usr1), Ok(mut usr2)) = (signal(SignalKind::user_defined1()), signal(SignalKind::user_defined2())) else { return };
                    loop {
                        tokio::select! {
                            _ = usr1.recv() => alternar(&h),
                            _ = usr2.recv() => { let _ = h.emit("atalho:reuniao", ()); }
                        }
                    }
                });
            }

            if let Err(e) = criar_bandeja(app) {
                eprintln!("Não foi possível criar o ícone na bandeja: {e}");
            }

            if let Some(j) = app.get_webview_window("main") {
                posicionar(&j);
                mostrar(&j);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            buscar, conversa, esconder, modo_mini, mostrar_janela, sair_app, config_ler, config_salvar, dispositivos_listar,
            material_ler, material_salvar, documentos_listar, documento_remover, anexar_arquivo,
            materiais_listar, material_guardar, material_carregar, material_excluir,
            abrir_link, atualizacao_verificar, atualizacao_instalar, chatgpt_conectar, chatgpt_desconectar, drive_estado, drive_conectar, drive_desconectar, drive_buscar, drive_anexar,
            locais_listar, local_baixar, local_cancelar, local_apagar, local_testar,
            perguntar, reuniao_iniciar, reuniao_parar, reuniao_reenviar,
            traducao_idiomas, traducao_iniciar, traducao_parar, traducao_enviar_email, traducao_limpar,
            pendencias_listar, pendencia_marcar,
            conversa_apagar, conversas_apagar_todas, conversas_apagar_antigas,
            exportar_texto, exportar_arquivo, backup_exportar
        ])
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o aplicativo")
        .run(|app, evento| {
            if let tauri::RunEvent::Exit = evento {
                // não deixa whisper-server ou llama-server rodando depois que o app fecha
                app.state::<Estado>().locais.parar_tudo();
            }
        });
}
