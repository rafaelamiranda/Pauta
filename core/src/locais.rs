//! Modelos locais (opcionais): transcrição com whisper.cpp e análise com llama.cpp.
//! Baixa o programa e o modelo para a pasta de dados do app, mostra o progresso e sobe o servidor sob demanda.

use crate::baixar::{baixar, caminho_parcial, Esperado, Progresso};
use crate::meeting::Emitir;
use crate::servidor::{ProcessoServidor, PORTA_LLM, PORTA_WHISPER};
use crate::settings::Config;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tipo {
    Whisper,
    Llm,
}

pub struct ModeloCatalogo {
    pub id: &'static str,
    pub tipo: Tipo,
    pub nome: &'static str,
    pub descricao: &'static str,
    pub arquivo: &'static str,
    /// Repositório no Hugging Face.
    pub repo: &'static str,
    /// Tamanho do arquivo em bytes (conferido no download contra o que o Hugging Face informa).
    pub tamanho: u64,
    pub licenca: &'static str,
    pub recomendado: bool,
}

/// Modelos oferecidos. Tamanhos e licenças conferidos na API do Hugging Face.
pub const CATALOGO: &[ModeloCatalogo] = &[
    ModeloCatalogo {
        id: "whisper-base", tipo: Tipo::Whisper, nome: "Whisper base", arquivo: "ggml-base.bin", repo: "ggerganov/whisper.cpp",
        tamanho: 147_951_465, licenca: "MIT", recomendado: false,
        descricao: "O mais leve e rápido. Costuma errar mais palavras.",
    },
    ModeloCatalogo {
        id: "whisper-small", tipo: Tipo::Whisper, nome: "Whisper small", arquivo: "ggml-small.bin", repo: "ggerganov/whisper.cpp",
        tamanho: 487_601_967, licenca: "MIT", recomendado: true,
        descricao: "Ponto de partida: tamanho moderado e boa qualidade para reuniões.",
    },
    ModeloCatalogo {
        id: "whisper-medium-q5", tipo: Tipo::Whisper, nome: "Whisper medium (compactado)", arquivo: "ggml-medium-q5_0.bin", repo: "ggerganov/whisper.cpp",
        tamanho: 539_212_467, licenca: "MIT", recomendado: false,
        descricao: "Maior que o small, em versão compactada (q5_0). Mais lento no processador.",
    },
    ModeloCatalogo {
        id: "whisper-turbo-q5", tipo: Tipo::Whisper, nome: "Whisper large v3 turbo (compactado)", arquivo: "ggml-large-v3-turbo-q5_0.bin", repo: "ggerganov/whisper.cpp",
        tamanho: 574_041_195, licenca: "MIT", recomendado: false,
        descricao: "O mais capaz da lista, em versão compactada (q5_0). O mais pesado para o processador.",
    },
    ModeloCatalogo {
        id: "qwen3-4b-q4", tipo: Tipo::Llm, nome: "Qwen3 4B Instruct 2507 (Q4_K_M)", arquivo: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf", repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
        tamanho: 2_497_281_120, licenca: "Apache 2.0", recomendado: true,
        descricao: "Resumo, pendências e respostas no seu computador. Versão compactada: menor e mais rápida.",
    },
    ModeloCatalogo {
        id: "qwen3-4b-q8", tipo: Tipo::Llm, nome: "Qwen3 4B Instruct 2507 (Q8_0)", arquivo: "Qwen3-4B-Instruct-2507-Q8_0.gguf", repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
        tamanho: 4_280_405_600, licenca: "Apache 2.0", recomendado: false,
        descricao: "Mesmo modelo com menos compactação: usa mais disco e memória.",
    },
    ModeloCatalogo {
        id: "llama31-8b-q4", tipo: Tipo::Llm, nome: "Llama 3.1 8B Instruct (Q4_K_M)", arquivo: "Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf", repo: "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
        tamanho: 4_920_739_232, licenca: "Llama 3.1 Community License", recomendado: false,
        descricao: "Modelo da Meta, maior que o Qwen3 4B: pode responder melhor, mas usa mais disco e memória e é mais lento no processador.",
    },
];

pub fn modelo(id: &str) -> Option<&'static ModeloCatalogo> {
    CATALOGO.iter().find(|m| m.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Programa {
    Whisper,
    LlamaCpu,
    LlamaVulkan,
}

impl Programa {
    pub fn do_modelo(m: &ModeloCatalogo, aceleracao: &str) -> Self {
        match (m.tipo, aceleracao) {
            (Tipo::Whisper, _) => Programa::Whisper,
            (Tipo::Llm, "vulkan") => Programa::LlamaVulkan,
            (Tipo::Llm, _) => Programa::LlamaCpu,
        }
    }

    fn pasta(self) -> &'static str {
        match self {
            Programa::Whisper => "whisper-cpp",
            Programa::LlamaCpu => "llama-cpu",
            Programa::LlamaVulkan => "llama-vulkan",
        }
    }

    fn exe(self) -> &'static str {
        match (self == Programa::Whisper, cfg!(windows)) {
            (true, true) => "whisper-server.exe",
            (true, false) => "whisper-server",
            (false, true) => "llama-server.exe",
            (false, false) => "llama-server",
        }
    }

    fn repo_github(self) -> &'static str {
        if self == Programa::Whisper { "ggml-org/whisper.cpp" } else { "ggml-org/llama.cpp" }
    }

    /// Este arquivo da versão publicada é o programa para o sistema atual?
    #[cfg(windows)]
    fn serve(self, nome: &str) -> bool {
        match self {
            Programa::Whisper => nome == "whisper-bin-x64.zip",
            Programa::LlamaCpu => nome.starts_with("llama-") && nome.ends_with("-bin-win-cpu-x64.zip"),
            Programa::LlamaVulkan => nome.starts_with("llama-") && nome.ends_with("-bin-win-vulkan-x64.zip"),
        }
    }

    #[cfg(not(windows))]
    fn serve(self, nome: &str) -> bool {
        let arq = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
        match self {
            Programa::Whisper => nome == format!("whisper-bin-ubuntu-{arq}.tar.gz"),
            Programa::LlamaCpu => nome.starts_with("llama-") && nome.ends_with(&format!("-bin-ubuntu-{arq}.tar.gz")),
            Programa::LlamaVulkan => nome.starts_with("llama-") && nome.ends_with(&format!("-bin-ubuntu-vulkan-{arq}.tar.gz")),
        }
    }
}

/// Arquivo de uma versão publicada no GitHub.
#[derive(Debug, PartialEq)]
struct Asset {
    nome: String,
    url: String,
    tamanho: Option<u64>,
    sha256: Option<String>,
}

/// Procura, nas versões mais recentes primeiro, o arquivo do programa para o sistema atual.
fn escolher_asset(prog: Programa, releases: &Value) -> Option<Asset> {
    releases.as_array()?.iter().flat_map(|r| r["assets"].as_array().into_iter().flatten()).find_map(|a| {
        let nome = a["name"].as_str()?;
        prog.serve(nome).then(|| Asset {
            nome: nome.to_string(),
            url: a["browser_download_url"].as_str().unwrap_or_default().to_string(),
            tamanho: a["size"].as_u64(),
            sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_string),
        })
    })
}

/// Extrai o zip em `destino`. Se todos os arquivos estiverem numa única pasta, tira essa pasta.
/// Recusa caminhos que saiam de `destino`.
fn extrair_zip(zip: &Path, destino: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip)?).map_err(|e| anyhow!("O arquivo baixado não é um zip válido: {e}"))?;
    let nomes: Vec<PathBuf> = (0..z.len()).filter_map(|i| z.by_index(i).ok().and_then(|f| f.enclosed_name())).collect();
    let topo = nomes.first().and_then(|n| n.components().next()).map(|c| c.as_os_str().to_owned());
    let pasta_unica = topo.is_some() && nomes.iter().all(|n| n.components().count() > 1 && n.components().next().map(|c| c.as_os_str().to_owned()) == topo);
    let tmp = destino.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let Some(rel) = f.enclosed_name() else { return Err(anyhow!("O zip tem um caminho inseguro e foi recusado.")) };
        let rel: PathBuf = if pasta_unica { rel.components().skip(1).collect() } else { rel };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let alvo = tmp.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&alvo)?;
        } else {
            if let Some(pai) = alvo.parent() {
                std::fs::create_dir_all(pai)?;
            }
            let mut saida = std::fs::File::create(&alvo)?;
            std::io::copy(&mut f, &mut saida)?;
        }
    }
    let _ = std::fs::remove_dir_all(destino);
    std::fs::rename(&tmp, destino)?;
    Ok(())
}

/// Extrai o .tar.gz em `destino`, mantendo as permissões (o servidor precisa ser executável) e os links das
/// bibliotecas. Se tudo estiver numa única pasta, tira essa pasta. Caminhos que saiam de `destino` são ignorados pelo `tar`.
fn extrair_tar_gz(arquivo: &Path, destino: &Path) -> Result<()> {
    let tmp = destino.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(arquivo)?));
    tar.set_preserve_permissions(true);
    if let Err(e) = tar.unpack(&tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(anyhow!("O arquivo baixado não é um .tar.gz válido: {e}"));
    }
    let filhos: Vec<PathBuf> = std::fs::read_dir(&tmp)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    let origem = match filhos.as_slice() {
        [unico] if unico.is_dir() => unico.clone(),
        _ => tmp.clone(),
    };
    let _ = std::fs::remove_dir_all(destino);
    std::fs::rename(&origem, destino)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Extrai o programa baixado (.zip no Windows, .tar.gz no Linux).
fn extrair_programa(arquivo: &Path, destino: &Path) -> Result<()> {
    if arquivo.to_string_lossy().ends_with(".tar.gz") { extrair_tar_gz(arquivo, destino) } else { extrair_zip(arquivo, destino) }
}

#[derive(Clone)]
enum Estado {
    Baixando { fase: &'static str, baixado: u64, total: u64 },
    Erro(String),
}

struct Trabalho {
    cancelar: Arc<AtomicBool>,
    estado: Estado,
}

struct Ativo {
    chave: String,
    proc: ProcessoServidor,
}

pub struct Locais {
    raiz: PathBuf,
    http: reqwest::Client,
    url_github: String,
    url_hf: String,
    emitir: Emitir,
    espera: Duration,
    trabalhos: Mutex<HashMap<String, Trabalho>>,
    whisper: tokio::sync::Mutex<Option<Ativo>>,
    llm: tokio::sync::Mutex<Option<Ativo>>,
}

impl Locais {
    pub fn new(raiz: PathBuf, emitir: Emitir) -> Arc<Self> {
        Self::com_urls(raiz, emitir, "https://api.github.com", "https://huggingface.co", Duration::from_secs(2))
    }

    pub fn com_urls(raiz: PathBuf, emitir: Emitir, github: &str, hf: &str, espera: Duration) -> Arc<Self> {
        Arc::new(Self {
            raiz,
            http: reqwest::Client::builder().user_agent("busca-overlay").build().unwrap_or_default(),
            url_github: github.into(),
            url_hf: hf.into(),
            emitir,
            espera,
            trabalhos: Mutex::new(HashMap::new()),
            whisper: tokio::sync::Mutex::new(None),
            llm: tokio::sync::Mutex::new(None),
        })
    }

    fn pasta_bin(&self, p: Programa) -> PathBuf {
        self.raiz.join(p.pasta()).join("bin")
    }

    fn exe(&self, p: Programa) -> PathBuf {
        self.pasta_bin(p).join(p.exe())
    }

    pub fn caminho_modelo(&self, m: &ModeloCatalogo) -> PathBuf {
        self.raiz.join("modelos").join(m.arquivo)
    }

    pub fn modelo_pronto(&self, m: &ModeloCatalogo) -> bool {
        self.caminho_modelo(m).exists()
    }

    fn registrar(&self, id: &str, fase: &'static str, baixado: u64, total: u64) {
        if let Some(t) = self.trabalhos.lock().unwrap().get_mut(id) {
            t.estado = Estado::Baixando { fase, baixado, total };
        }
        (self.emitir)("local:progresso", json!({"id": id, "estado": "baixando", "fase": fase, "baixado": baixado, "total": total}));
    }

    /// Situação de todos os modelos do catálogo, para a tela.
    pub fn listar(&self, aceleracao: &str) -> Value {
        let trabalhos = self.trabalhos.lock().unwrap();
        Value::Array(
            CATALOGO
                .iter()
                .map(|m| {
                    let mut v = json!({
                        "id": m.id, "tipo": m.tipo, "nome": m.nome, "descricao": m.descricao, "tamanho": m.tamanho,
                        "licenca": m.licenca, "recomendado": m.recomendado, "estado": "ausente", "baixado": 0, "total": 0,
                        "programaInstalado": self.exe(Programa::do_modelo(m, aceleracao)).exists(),
                    });
                    let parcial = std::fs::metadata(caminho_parcial(&self.caminho_modelo(m))).map(|f| f.len()).unwrap_or(0);
                    match trabalhos.get(m.id).map(|t| &t.estado) {
                        Some(Estado::Baixando { fase, baixado, total }) => {
                            v["estado"] = "baixando".into();
                            v["fase"] = (*fase).into();
                            v["baixado"] = (*baixado).into();
                            v["total"] = (*total).into();
                        }
                        Some(Estado::Erro(e)) => {
                            v["estado"] = "erro".into();
                            v["erro"] = e.clone().into();
                            v["baixado"] = parcial.into();
                        }
                        None if self.modelo_pronto(m) => v["estado"] = "pronto".into(),
                        None if parcial > 0 => {
                            v["estado"] = "parcial".into();
                            v["baixado"] = parcial.into();
                        }
                        None => {}
                    }
                    v
                })
                .collect(),
        )
    }

    async fn json_get(&self, url: &str) -> Result<Value> {
        let res = self.http.get(url).send().await.map_err(|e| anyhow!("Sem conexão: {e}"))?;
        let status = res.status();
        if !status.is_success() {
            return Err(anyhow!("{url} respondeu {}.", status.as_u16()));
        }
        Ok(res.json().await?)
    }

    /// Tamanho e hash do arquivo, como o Hugging Face informa. Se a consulta falhar, usa o tamanho do catálogo.
    async fn esperado_do_modelo(&self, m: &ModeloCatalogo) -> Esperado {
        let info = self.json_get(&format!("{}/api/models/{}/tree/main", self.url_hf, m.repo)).await.ok();
        let arq = info.as_ref().and_then(|j| j.as_array()).and_then(|a| a.iter().find(|f| f["path"] == m.arquivo));
        Esperado {
            tamanho: arq.and_then(|f| f["size"].as_u64()).or(Some(m.tamanho)),
            sha256: arq.and_then(|f| f["lfs"]["oid"].as_str()).map(str::to_string),
        }
    }

    fn progresso(self: &Arc<Self>, id: &str, fase: &'static str) -> Progresso {
        let (eu, id) = (self.clone(), id.to_string());
        Arc::new(move |b, t| eu.registrar(&id, fase, b, t))
    }

    async fn instalar_programa(self: &Arc<Self>, id: &str, prog: Programa, cancelar: &AtomicBool) -> Result<()> {
        self.registrar(id, "programa", 0, 0);
        let releases = self.json_get(&format!("{}/repos/{}/releases?per_page=30", self.url_github, prog.repo_github())).await?;
        let asset = escolher_asset(prog, &releases).ok_or_else(|| anyhow!("Não achei o programa para {} nas versões recentes de {}.", if cfg!(windows) { "Windows" } else { "Linux" }, prog.repo_github()))?;
        let zip = self.raiz.join("temporario").join(&asset.nome);
        let esperado = Esperado { tamanho: asset.tamanho, sha256: asset.sha256.clone() };
        baixar(&self.http, &asset.url, &zip, &esperado, &self.progresso(id, "programa"), cancelar, self.espera).await?;
        let (z, d) = (zip.clone(), self.pasta_bin(prog));
        tokio::task::spawn_blocking(move || extrair_programa(&z, &d)).await??;
        let _ = std::fs::remove_file(&zip);
        if !self.exe(prog).exists() {
            return Err(anyhow!("O programa baixado não contém o {}.", prog.exe()));
        }
        Ok(())
    }

    async fn executar_download(self: &Arc<Self>, m: &'static ModeloCatalogo, aceleracao: &str, cancelar: &AtomicBool) -> Result<()> {
        let prog = Programa::do_modelo(m, aceleracao);
        if !self.exe(prog).exists() {
            self.instalar_programa(m.id, prog, cancelar).await?;
        }
        let destino = self.caminho_modelo(m);
        if destino.exists() {
            return Ok(());
        }
        self.registrar(m.id, "modelo", 0, m.tamanho);
        let esperado = self.esperado_do_modelo(m).await;
        let url = format!("{}/{}/resolve/main/{}", self.url_hf, m.repo, m.arquivo);
        baixar(&self.http, &url, &destino, &esperado, &self.progresso(m.id, "modelo"), cancelar, self.espera).await
    }

    /// Começa a baixar o programa (se faltar) e o modelo, em segundo plano. Progresso pelo evento `local:progresso`.
    pub fn iniciar_download(self: &Arc<Self>, id: &str, aceleracao: &str) -> Result<()> {
        let m = modelo(id).ok_or_else(|| anyhow!("Modelo desconhecido."))?;
        let cancelar = Arc::new(AtomicBool::new(false));
        {
            let mut t = self.trabalhos.lock().unwrap();
            if matches!(t.get(id).map(|t| &t.estado), Some(Estado::Baixando { .. })) {
                return Err(anyhow!("Este modelo já está sendo baixado."));
            }
            t.insert(id.to_string(), Trabalho { cancelar: cancelar.clone(), estado: Estado::Baixando { fase: "programa", baixado: 0, total: 0 } });
        }
        let (eu, aceleracao) = (self.clone(), aceleracao.to_string());
        tokio::spawn(async move {
            let r = eu.executar_download(m, &aceleracao, &cancelar).await;
            match r {
                Ok(()) => {
                    eu.trabalhos.lock().unwrap().remove(m.id);
                    (eu.emitir)("local:progresso", json!({"id": m.id, "estado": "pronto"}));
                }
                Err(_) if cancelar.load(Ordering::Relaxed) => {
                    eu.trabalhos.lock().unwrap().remove(m.id);
                    (eu.emitir)("local:progresso", json!({"id": m.id, "estado": "cancelado"}));
                }
                Err(e) => {
                    let msg = e.to_string();
                    eu.trabalhos.lock().unwrap().insert(m.id.to_string(), Trabalho { cancelar: cancelar.clone(), estado: Estado::Erro(msg.clone()) });
                    (eu.emitir)("local:progresso", json!({"id": m.id, "estado": "erro", "mensagem": msg}));
                }
            }
        });
        Ok(())
    }

    /// Cancela o download em andamento e apaga o arquivo parcial.
    pub fn cancelar(&self, id: &str) {
        if let Some(t) = self.trabalhos.lock().unwrap().get(id) {
            t.cancelar.store(true, Ordering::Relaxed);
        }
    }

    /// Apaga o modelo baixado (e o parcial). Se ele estiver em uso, desliga o servidor antes.
    pub async fn apagar(&self, id: &str) -> Result<()> {
        let m = modelo(id).ok_or_else(|| anyhow!("Modelo desconhecido."))?;
        if matches!(self.trabalhos.lock().unwrap().get(id).map(|t| &t.estado), Some(Estado::Baixando { .. })) {
            return Err(anyhow!("Cancele o download antes de apagar."));
        }
        match m.tipo {
            Tipo::Whisper => *self.whisper.lock().await = None,
            Tipo::Llm => *self.llm.lock().await = None,
        }
        let caminho = self.caminho_modelo(m);
        for c in [caminho_parcial(&caminho), caminho] {
            match std::fs::remove_file(&c) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(anyhow!("Não foi possível apagar o arquivo: {e}")),
            }
        }
        self.trabalhos.lock().unwrap().remove(id);
        Ok(())
    }

    /// Garante o servidor de transcrição rodando com o modelo e o idioma dos Ajustes. Devolve a URL.
    pub async fn garantir_whisper(&self, cfg: &Config) -> Result<String> {
        let m = modelo(&cfg.whisper_local).filter(|m| m.tipo == Tipo::Whisper).ok_or_else(|| anyhow!("Escolha um modelo de voz local nos Ajustes."))?;
        if !self.modelo_pronto(m) {
            return Err(anyhow!("O modelo de voz local ({}) ainda não foi baixado. Baixe nos Ajustes ou volte para a OpenAI.", m.nome));
        }
        let chave = format!("{}|{}", m.id, cfg.idioma);
        let mut g = self.whisper.lock().await;
        if let Some(a) = g.as_ref().filter(|a| a.chave == chave && a.proc.vivo()) {
            return Ok(a.proc.url());
        }
        *g = None;
        let proc = ProcessoServidor::novo(
            "whisper-server",
            self.exe(Programa::Whisper),
            vec![
                "-m".into(), self.caminho_modelo(m).to_string_lossy().into_owned(),
                "--host".into(), "127.0.0.1".into(), "--port".into(), PORTA_WHISPER.to_string(), "-l".into(), cfg.idioma.clone(),
            ],
            PORTA_WHISPER, None, Duration::from_secs(90), self.raiz.join("logs").join("whisper-server.log"),
        );
        proc.iniciar().await?;
        let url = proc.url();
        *g = Some(Ativo { chave, proc });
        Ok(url)
    }

    /// Garante o servidor do modelo de análise rodando. Devolve a URL base da API (com /v1).
    pub async fn garantir_llm(&self, cfg: &Config) -> Result<String> {
        let m = modelo(&cfg.llm_local).filter(|m| m.tipo == Tipo::Llm).ok_or_else(|| anyhow!("Escolha um modelo de análise local nos Ajustes."))?;
        if !self.modelo_pronto(m) {
            return Err(anyhow!("O modelo de análise local ({}) ainda não foi baixado. Baixe nos Ajustes ou volte para a OpenAI.", m.nome));
        }
        let prog = Programa::do_modelo(m, &cfg.aceleracao);
        let chave = format!("{}|{:?}", m.id, prog);
        let mut g = self.llm.lock().await;
        if let Some(a) = g.as_ref().filter(|a| a.chave == chave && a.proc.vivo()) {
            return Ok(format!("{}/v1", a.proc.url()));
        }
        *g = None;
        let mut args: Vec<String> = vec![
            "-m".into(), self.caminho_modelo(m).to_string_lossy().into_owned(),
            "--host".into(), "127.0.0.1".into(), "--port".into(), PORTA_LLM.to_string(),
            "-c".into(), "32768".into(), "-np".into(), "1".into(),
        ];
        if prog == Programa::LlamaVulkan {
            args.extend(["-ngl".into(), "99".into()]);
        }
        let proc = ProcessoServidor::novo(
            "llama-server", self.exe(prog), args, PORTA_LLM, Some("/health"), Duration::from_secs(240),
            self.raiz.join("logs").join("llama-server.log"),
        );
        proc.iniciar().await?;
        let url = format!("{}/v1", proc.url());
        *g = Some(Ativo { chave, proc });
        Ok(url)
    }

    /// Desliga os servidores que os Ajustes não usam mais, para liberar memória.
    pub async fn liberar_nao_usados(&self, cfg: &Config) {
        if !cfg.transcricao_local {
            *self.whisper.lock().await = None;
        }
        if !cfg.analise_local {
            *self.llm.lock().await = None;
        }
    }

    /// Desliga tudo (ao fechar o app). Se algum servidor estiver ocupado iniciando, o sistema o encerra junto com o app.
    pub fn parar_tudo(&self) {
        if let Ok(mut g) = self.whisper.try_lock() {
            *g = None;
        }
        if let Ok(mut g) = self.llm.try_lock() {
            *g = None;
        }
    }

    /// Sobe o servidor do modelo de análise e faz uma pergunta curta. Devolve o tempo gasto em segundos.
    pub async fn testar_llm(&self, cfg: &Config) -> Result<f64> {
        let inicio = std::time::Instant::now();
        let url = self.garantir_llm(cfg).await?;
        let res = self
            .http
            .post(format!("{url}/chat/completions"))
            .json(&json!({"model": "local", "max_tokens": 16, "messages": [{"role": "user", "content": "Responda apenas: ok"}]}))
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!("O servidor local respondeu {}.", res.status().as_u16()));
        }
        Ok(inicio.elapsed().as_secs_f64())
    }

    /// Sobe o servidor de transcrição. Devolve o tempo gasto em segundos.
    pub async fn testar_whisper(&self, cfg: &Config) -> Result<f64> {
        let inicio = std::time::Instant::now();
        self.garantir_whisper(cfg).await?;
        Ok(inicio.elapsed().as_secs_f64())
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn zip_com(arquivos: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (nome, dados) in arquivos {
                z.start_file(*nome, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(dados).unwrap();
            }
            z.finish().unwrap();
        }
        buf
    }

    fn ler(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    fn pasta() -> PathBuf {
        let p = std::env::temp_dir().join(format!("locais-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Servidor com rotas: responde o corpo da primeira rota cujo prefixo casa com o caminho pedido.
    async fn servidor_rotas(rotas: Vec<(String, Vec<u8>)>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let rotas = rotas.clone();
                tokio::spawn(async move {
                    let mut b = vec![0u8; 4096];
                    let n = s.read(&mut b).await.unwrap_or(0);
                    let pedido = String::from_utf8_lossy(&b[..n]).to_string();
                    let caminho = pedido.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (status, corpo) = match rotas.iter().find(|(p, _)| caminho.starts_with(p)) {
                        Some((_, c)) => ("200 OK", c.clone()),
                        None => ("404 Not Found", vec![]),
                    };
                    let cab = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", corpo.len());
                    let _ = s.write_all(cab.as_bytes()).await;
                    let _ = s.write_all(&corpo).await;
                });
            }
        });
        url
    }

    fn sem_eventos() -> (Emitir, Arc<Mutex<Vec<Value>>>) {
        let v: Arc<Mutex<Vec<Value>>> = Default::default();
        let v2 = v.clone();
        (Arc::new(move |_, d| v2.lock().unwrap().push(d)), v)
    }

    #[test]
    fn catalogo_tem_ids_unicos_e_o_programa_certo() {
        let mut ids: Vec<_> = CATALOGO.iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGO.len());
        assert!(CATALOGO.iter().any(|m| m.tipo == Tipo::Whisper && m.recomendado));
        assert!(CATALOGO.iter().any(|m| m.tipo == Tipo::Llm && m.recomendado));
        let llm = modelo("qwen3-4b-q4").unwrap();
        assert_eq!(Programa::do_modelo(llm, "vulkan"), Programa::LlamaVulkan);
        assert_eq!(Programa::do_modelo(llm, "cpu"), Programa::LlamaCpu);
        assert_eq!(Programa::do_modelo(llm, "qualquer"), Programa::LlamaCpu);
        assert_eq!(Programa::do_modelo(modelo("whisper-small").unwrap(), "vulkan"), Programa::Whisper);
    }

    #[cfg(windows)]
    const NOMES: [&str; 4] = ["llama-b2-bin-win-cuda-12.4-x64.zip", "llama-b2-bin-win-cpu-x64.zip", "llama-b2-bin-win-vulkan-x64.zip", "llama-b1-bin-win-cpu-x64.zip"];
    #[cfg(not(windows))]
    const NOMES: [&str; 4] = [
        "llama-b2-bin-ubuntu-cuda-12.8-x64.tar.gz",
        "llama-b2-bin-ubuntu-x64.tar.gz",
        "llama-b2-bin-ubuntu-vulkan-x64.tar.gz",
        "llama-b1-bin-ubuntu-x64.tar.gz",
    ];

    #[test]
    fn escolhe_o_arquivo_certo_da_versao_mais_recente() {
        let r = json!([
            {"tag_name": "v2", "assets": []},
            {"tag_name": "b2", "assets": [
                {"name": NOMES[0], "browser_download_url": "u0", "size": 1},
                {"name": NOMES[1], "browser_download_url": "u1", "size": 10, "digest": "sha256:abc"},
                {"name": NOMES[2], "browser_download_url": "u2", "size": 20},
            ]},
            {"tag_name": "b1", "assets": [{"name": NOMES[3], "browser_download_url": "velho", "size": 5}]},
        ]);
        let cpu = escolher_asset(Programa::LlamaCpu, &r).unwrap();
        assert_eq!((cpu.url.as_str(), cpu.tamanho, cpu.sha256.as_deref()), ("u1", Some(10), Some("abc")));
        assert_eq!(escolher_asset(Programa::LlamaVulkan, &r).unwrap().url, "u2");
        assert!(escolher_asset(Programa::Whisper, &r).is_none());
        assert!(escolher_asset(Programa::Whisper, &json!({})).is_none());
    }

    #[test]
    fn zip_com_pasta_unica_tem_a_pasta_removida() {
        let p = pasta();
        let zip = p.join("a.zip");
        std::fs::write(&zip, zip_com(&[("Release/whisper-server.exe", b"exe"), ("Release/ggml.dll", b"dll")])).unwrap();
        extrair_zip(&zip, &p.join("bin")).unwrap();
        assert_eq!(ler(&p.join("bin/whisper-server.exe")), "exe");
        assert!(p.join("bin/ggml.dll").exists());
        assert!(!p.join("bin.tmp").exists());

        std::fs::write(&zip, zip_com(&[("llama-server.exe", b"x"), ("ggml.dll", b"y")])).unwrap();
        extrair_zip(&zip, &p.join("bin2")).unwrap();
        assert!(p.join("bin2/llama-server.exe").exists(), "zip sem pasta fica como está");
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn zip_com_caminho_fora_da_pasta_e_recusado() {
        let p = pasta();
        let zip = p.join("mau.zip");
        std::fs::write(&zip, zip_com(&[("../fora.txt", b"x"), ("ok.txt", b"y")])).unwrap();
        assert!(extrair_zip(&zip, &p.join("bin")).is_err());
        assert!(!p.join("fora.txt").exists());
        std::fs::write(&zip, b"nao e zip").unwrap();
        assert!(extrair_zip(&zip, &p.join("bin")).unwrap_err().to_string().contains("zip válido"));
        let _ = std::fs::remove_dir_all(p);
    }

    #[cfg(unix)]
    fn targz_com(pasta: Option<&str>, arquivos: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (nome, dados, modo) in arquivos {
            let mut h = tar::Header::new_gnu();
            h.set_size(dados.len() as u64);
            h.set_mode(*modo);
            h.set_cksum();
            let caminho = pasta.map(|p| format!("{p}/{nome}")).unwrap_or_else(|| nome.to_string());
            tar.append_data(&mut h, caminho, *dados).unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        gz.finish().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn tar_gz_com_pasta_unica_tem_a_pasta_removida_e_mantem_o_executavel() {
        use std::os::unix::fs::PermissionsExt;
        let p = pasta();
        let arq = p.join("a.tar.gz");
        std::fs::write(&arq, targz_com(Some("whisper-bin-ubuntu-x64"), &[("whisper-server", b"elf", 0o755), ("libggml.so", b"so", 0o644)])).unwrap();
        extrair_programa(&arq, &p.join("bin")).unwrap();
        assert_eq!(ler(&p.join("bin/whisper-server")), "elf");
        assert!(p.join("bin/libggml.so").exists());
        assert_ne!(std::fs::metadata(p.join("bin/whisper-server")).unwrap().permissions().mode() & 0o111, 0, "perdeu o bit de executável");
        assert!(!p.join("bin.tmp").exists());

        std::fs::write(&arq, targz_com(None, &[("llama-server", b"x", 0o755), ("outro", b"y", 0o644)])).unwrap();
        extrair_programa(&arq, &p.join("bin2")).unwrap();
        assert!(p.join("bin2/llama-server").exists(), "sem pasta única fica como está");

        std::fs::write(&arq, b"nao e tar").unwrap();
        assert!(extrair_programa(&arq, &p.join("bin3")).unwrap_err().to_string().contains(".tar.gz válido"));
        let _ = std::fs::remove_dir_all(p);
    }

    #[tokio::test]
    async fn baixa_programa_e_modelo_confere_e_lista_como_pronto() {
        let raiz = pasta();
        let modelo_bytes: Vec<u8> = (0..5000u32).map(|i| (i % 200) as u8).collect();
        let tmp = raiz.join("tmp.bin");
        std::fs::write(&tmp, &modelo_bytes).unwrap();
        let hash = crate::baixar::sha256_do_arquivo(&tmp).unwrap();
        #[cfg(windows)]
        let (nome_arq, zip) = ("whisper-bin-x64.zip", zip_com(&[("Release/whisper-server.exe", b"MZ")]));
        #[cfg(not(windows))]
        let (nome_arq, zip) = (
            if cfg!(target_arch = "aarch64") { "whisper-bin-ubuntu-arm64.tar.gz" } else { "whisper-bin-ubuntu-x64.tar.gz" },
            targz_com(Some("whisper-bin-ubuntu-x64"), &[("whisper-server", b"elf", 0o755)]),
        );
        // um só servidor faz de GitHub, Hugging Face e arquivos
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let releases = json!([{"assets": [{"name": nome_arq, "browser_download_url": format!("{base}/zip/whisper.zip"), "size": zip.len()}]}]);
        let arvore = json!([{"path": "ggml-base.bin", "size": modelo_bytes.len(), "lfs": {"oid": hash}}]);
        let rotas: Vec<(String, Vec<u8>)> = vec![
            ("/repos/ggml-org/whisper.cpp/releases".into(), releases.to_string().into_bytes()),
            ("/zip/whisper.zip".into(), zip),
            ("/api/models/ggerganov/whisper.cpp/tree/main".into(), arvore.to_string().into_bytes()),
            ("/ggerganov/whisper.cpp/resolve/main/ggml-base.bin".into(), modelo_bytes.clone()),
        ];
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let rotas = rotas.clone();
                tokio::spawn(async move {
                    let mut b = vec![0u8; 4096];
                    let n = s.read(&mut b).await.unwrap_or(0);
                    let caminho = String::from_utf8_lossy(&b[..n]).split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (st, corpo) = match rotas.iter().find(|(p, _)| caminho.starts_with(p)) {
                        Some((_, c)) => ("200 OK", c.clone()),
                        None => ("404 Not Found", vec![]),
                    };
                    let _ = s.write_all(format!("HTTP/1.1 {st}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", corpo.len()).as_bytes()).await;
                    let _ = s.write_all(&corpo).await;
                });
            }
        });

        let (emitir, eventos) = sem_eventos();
        let locais = Locais::com_urls(raiz.clone(), emitir, &base, &base, Duration::ZERO);
        let estado_de = |l: &Locais, id: &str| l.listar("cpu").as_array().unwrap().iter().find(|m| m["id"] == id).unwrap()["estado"].as_str().unwrap().to_string();
        assert_eq!(estado_de(&locais, "whisper-base"), "ausente");

        locais.iniciar_download("whisper-base", "cpu").unwrap();
        assert!(locais.iniciar_download("whisper-base", "cpu").unwrap_err().to_string().contains("já está"));
        for _ in 0..100 {
            if estado_de(&locais, "whisper-base") != "baixando" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(estado_de(&locais, "whisper-base"), "pronto", "eventos: {:?}", eventos.lock().unwrap());
        assert!(raiz.join("whisper-cpp/bin").join(Programa::Whisper.exe()).exists());
        assert_eq!(std::fs::read(raiz.join("modelos/ggml-base.bin")).unwrap(), modelo_bytes);
        let ev = eventos.lock().unwrap();
        assert!(ev.iter().any(|e| e["fase"] == "programa") && ev.iter().any(|e| e["fase"] == "modelo") && ev.last().unwrap()["estado"] == "pronto");
        drop(ev);

        locais.apagar("whisper-base").await.unwrap();
        assert_eq!(estado_de(&locais, "whisper-base"), "ausente");
        let _ = std::fs::remove_dir_all(raiz);
    }

    #[tokio::test]
    async fn falha_no_download_fica_como_erro_e_deixa_tentar_de_novo() {
        let raiz = pasta();
        let url = servidor_rotas(vec![]).await; // tudo dá 404
        let (emitir, eventos) = sem_eventos();
        let locais = Locais::com_urls(raiz.clone(), emitir, &url, &url, Duration::ZERO);
        locais.iniciar_download("qwen3-4b-q4", "cpu").unwrap();
        for _ in 0..100 {
            if eventos.lock().unwrap().iter().any(|e| e["estado"] == "erro") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let item = locais.listar("cpu").as_array().unwrap().iter().find(|m| m["id"] == "qwen3-4b-q4").unwrap().clone();
        assert_eq!(item["estado"], "erro");
        assert!(item["erro"].as_str().unwrap().contains("404"), "{item}");
        assert!(locais.iniciar_download("qwen3-4b-q4", "cpu").is_ok(), "erro anterior não bloqueia nova tentativa");
        assert!(locais.iniciar_download("nao-existe", "cpu").is_err());
        let _ = std::fs::remove_dir_all(raiz);
    }

    #[tokio::test]
    async fn sem_modelo_baixado_os_servidores_explicam_o_que_fazer() {
        let raiz = pasta();
        let (emitir, _) = sem_eventos();
        let locais = Locais::com_urls(raiz.clone(), emitir, "http://127.0.0.1:1", "http://127.0.0.1:1", Duration::ZERO);
        let cfg = Config::default();
        let e = locais.garantir_whisper(&cfg).await.unwrap_err().to_string();
        assert!(e.contains("ainda não foi baixado") && e.contains("OpenAI"), "{e}");
        let e = locais.garantir_llm(&cfg).await.unwrap_err().to_string();
        assert!(e.contains("ainda não foi baixado"), "{e}");
        let ruim = Config { llm_local: "whisper-small".into(), ..Default::default() };
        assert!(locais.garantir_llm(&ruim).await.unwrap_err().to_string().contains("Escolha"));
        assert!(locais.apagar("whisper-small").await.is_ok(), "apagar o que não existe não é erro");
        let _ = std::fs::remove_dir_all(raiz);
    }
}
