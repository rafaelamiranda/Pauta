//! Download de arquivos grandes: progresso, cancelamento, retomada e verificação.

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Chamado com (bytes baixados, total) enquanto o arquivo desce.
pub type Progresso = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// O que se sabe do arquivo antes de baixar. Se informado, é conferido no final.
#[derive(Debug, Clone, Default)]
pub struct Esperado {
    pub tamanho: Option<u64>,
    pub sha256: Option<String>,
}

const TENTATIVAS: u32 = 4;

pub fn caminho_parcial(destino: &Path) -> PathBuf {
    let mut nome = destino.file_name().unwrap_or_default().to_os_string();
    nome.push(".parte");
    destino.with_file_name(nome)
}

pub fn sha256_do_arquivo(caminho: &Path) -> Result<String> {
    let mut f = std::fs::File::open(caminho)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Baixa `url` para `destino`. Grava em `destino.parte` e só renomeia no final, depois de conferir
/// tamanho e hash. Se a conexão cair, tenta de novo de onde parou. Se falhar de vez, deixa o arquivo
/// parcial para a próxima chamada continuar. Se cancelado, apaga o parcial.
pub async fn baixar(
    http: &reqwest::Client,
    url: &str,
    destino: &Path,
    esperado: &Esperado,
    progresso: &Progresso,
    cancelar: &AtomicBool,
    espera_nova_tentativa: Duration,
) -> Result<()> {
    if let Some(pai) = destino.parent() {
        std::fs::create_dir_all(pai)?;
    }
    let parcial = caminho_parcial(destino);
    let mut ultimo_erro = anyhow!("Falha no download.");
    let mut zerou = false;

    for tentativa in 0..TENTATIVAS {
        if tentativa > 0 {
            tokio::time::sleep(espera_nova_tentativa).await;
        }
        let ja = std::fs::metadata(&parcial).map(|m| m.len()).unwrap_or(0);
        if esperado.tamanho.is_some_and(|t| ja == t) {
            break; // já está inteiro, falta só conferir
        }
        let mut pedido = http.get(url);
        if ja > 0 {
            pedido = pedido.header("Range", format!("bytes={ja}-"));
        }
        let mut res = match pedido.send().await {
            Ok(r) => r,
            Err(e) => {
                ultimo_erro = anyhow!("Sem conexão: {e}");
                continue;
            }
        };
        let status = res.status().as_u16();
        if status == 416 && !zerou {
            // o parcial não confere com o servidor: recomeça do zero
            let _ = std::fs::remove_file(&parcial);
            zerou = true;
            ultimo_erro = anyhow!("O arquivo parcial era inválido. Recomeçando.");
            continue;
        }
        if !res.status().is_success() {
            let e = anyhow!("O servidor respondeu {status}.");
            if res.status().is_client_error() && status != 429 {
                return Err(e);
            }
            ultimo_erro = e;
            continue;
        }
        let (mut baixado, anexar) = if status == 206 { (ja, true) } else { (0, false) };
        let total = res.content_length().map(|c| c + baixado).or(esperado.tamanho).unwrap_or(0);
        let mut arquivo = std::fs::OpenOptions::new().create(true).write(true).append(anexar).truncate(!anexar).open(&parcial)?;
        let mut ultimo_aviso = Instant::now() - Duration::from_secs(1);
        let mut falhou = None;
        loop {
            if cancelar.load(Ordering::Relaxed) {
                drop(arquivo);
                let _ = std::fs::remove_file(&parcial);
                return Err(anyhow!("Download cancelado."));
            }
            match res.chunk().await {
                Ok(Some(c)) => {
                    arquivo.write_all(&c)?;
                    baixado += c.len() as u64;
                    if ultimo_aviso.elapsed() >= Duration::from_millis(200) {
                        ultimo_aviso = Instant::now();
                        progresso(baixado, total);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    falhou = Some(anyhow!("A conexão caiu: {e}"));
                    break;
                }
            }
        }
        arquivo.flush()?;
        drop(arquivo);
        progresso(baixado, total);
        match falhou {
            Some(e) => ultimo_erro = e,
            None => {
                ultimo_erro = anyhow!("O download terminou antes do fim do arquivo.");
                // terminou sem erro: confere o tamanho abaixo
                break;
            }
        }
    }

    let tamanho = std::fs::metadata(&parcial).map(|m| m.len()).map_err(|_| ultimo_erro)?;
    if let Some(t) = esperado.tamanho {
        if tamanho != t {
            return Err(anyhow!("Download incompleto ({tamanho} de {t} bytes). Clique em Baixar para continuar de onde parou."));
        }
    }
    progresso(tamanho, tamanho);
    if let Some(h) = &esperado.sha256 {
        let p = parcial.clone();
        let calculado = tokio::task::spawn_blocking(move || sha256_do_arquivo(&p)).await??;
        if !calculado.eq_ignore_ascii_case(h) {
            let _ = std::fs::remove_file(&parcial);
            return Err(anyhow!("O arquivo baixado está corrompido (a verificação falhou). Tente baixar de novo."));
        }
    }
    let _ = std::fs::remove_file(destino);
    std::fs::rename(&parcial, destino)?;
    Ok(())
}

#[cfg(test)]
pub mod testes {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Servidor que entrega `dados` com suporte a Range. `cortar_primeira` faz a primeira resposta
    /// parar no meio (simula queda de conexão). Devolve a URL e os cabeçalhos Range recebidos.
    pub async fn servidor_de_arquivo(dados: Vec<u8>, cortar_primeira: Option<usize>) -> (String, Arc<Mutex<Vec<Option<u64>>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/arquivo.bin", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(vec![]));
        let r2 = ranges.clone();
        let contador = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let (dados, r2, contador) = (dados.clone(), r2.clone(), contador.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let cab = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let inicio = cab.split("range: bytes=").nth(1).and_then(|r| r.split('-').next()).and_then(|v| v.trim().parse::<u64>().ok());
                    r2.lock().unwrap().push(inicio);
                    let vez = contador.fetch_add(1, Ordering::SeqCst);
                    let ini = inicio.unwrap_or(0) as usize;
                    if ini >= dados.len() {
                        let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                    let corpo = &dados[ini..];
                    let status = if inicio.is_some() { "206 Partial Content" } else { "200 OK" };
                    let cab = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", corpo.len());
                    let _ = sock.write_all(cab.as_bytes()).await;
                    match cortar_primeira.filter(|_| vez == 0) {
                        Some(ate) => {
                            let _ = sock.write_all(&corpo[..ate.min(corpo.len())]).await;
                            let _ = sock.flush().await;
                        }
                        None => {
                            let _ = sock.write_all(corpo).await;
                        }
                    }
                });
            }
        });
        (url, ranges)
    }

    fn pasta() -> PathBuf {
        let p = std::env::temp_dir().join(format!("baixar-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn sem_progresso() -> Progresso {
        Arc::new(|_, _| {})
    }

    fn dados() -> Vec<u8> {
        (0..200_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn sha(d: &[u8]) -> String {
        Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[tokio::test]
    async fn baixa_confere_tamanho_e_hash_e_so_entao_renomeia() {
        let d = dados();
        let (url, _) = servidor_de_arquivo(d.clone(), None).await;
        let pasta = pasta();
        let destino = pasta.join("m.bin");
        let esperado = Esperado { tamanho: Some(d.len() as u64), sha256: Some(sha(&d)) };
        let vistos = Arc::new(Mutex::new(vec![]));
        let v2 = vistos.clone();
        let prog: Progresso = Arc::new(move |a, t| v2.lock().unwrap().push((a, t)));
        baixar(&reqwest::Client::new(), &url, &destino, &esperado, &prog, &AtomicBool::new(false), Duration::ZERO).await.unwrap();
        assert_eq!(std::fs::read(&destino).unwrap(), d);
        assert!(!caminho_parcial(&destino).exists());
        assert_eq!(*vistos.lock().unwrap().last().unwrap(), (d.len() as u64, d.len() as u64));
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn conexao_que_cai_continua_de_onde_parou() {
        let d = dados();
        let (url, ranges) = servidor_de_arquivo(d.clone(), Some(50_000)).await;
        let pasta = pasta();
        let destino = pasta.join("m.bin");
        let esperado = Esperado { tamanho: Some(d.len() as u64), sha256: Some(sha(&d)) };
        baixar(&reqwest::Client::new(), &url, &destino, &esperado, &sem_progresso(), &AtomicBool::new(false), Duration::ZERO).await.unwrap();
        assert_eq!(std::fs::read(&destino).unwrap(), d);
        let r = ranges.lock().unwrap().clone();
        assert_eq!(r[0], None);
        assert!(r[1].unwrap() > 0, "o segundo pedido usa Range: {r:?}");
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn parcial_de_uma_tentativa_anterior_e_retomado() {
        let d = dados();
        let (url, ranges) = servidor_de_arquivo(d.clone(), None).await;
        let pasta = pasta();
        let destino = pasta.join("m.bin");
        std::fs::write(caminho_parcial(&destino), &d[..80_000]).unwrap();
        let esperado = Esperado { tamanho: Some(d.len() as u64), sha256: Some(sha(&d)) };
        baixar(&reqwest::Client::new(), &url, &destino, &esperado, &sem_progresso(), &AtomicBool::new(false), Duration::ZERO).await.unwrap();
        assert_eq!(std::fs::read(&destino).unwrap(), d);
        assert_eq!(ranges.lock().unwrap()[0], Some(80_000));
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn arquivo_corrompido_e_recusado_e_apagado() {
        let d = dados();
        let (url, _) = servidor_de_arquivo(d.clone(), None).await;
        let pasta = pasta();
        let destino = pasta.join("m.bin");
        let esperado = Esperado { tamanho: Some(d.len() as u64), sha256: Some("0".repeat(64)) };
        let e = baixar(&reqwest::Client::new(), &url, &destino, &esperado, &sem_progresso(), &AtomicBool::new(false), Duration::ZERO).await.unwrap_err();
        assert!(e.to_string().contains("corrompido"), "{e}");
        assert!(!destino.exists() && !caminho_parcial(&destino).exists());
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn tamanho_diferente_do_esperado_e_recusado() {
        let d = dados();
        let (url, _) = servidor_de_arquivo(d.clone(), None).await;
        let pasta = pasta();
        let destino = pasta.join("m.bin");
        let esperado = Esperado { tamanho: Some(d.len() as u64 + 10), sha256: None };
        let e = baixar(&reqwest::Client::new(), &url, &destino, &esperado, &sem_progresso(), &AtomicBool::new(false), Duration::ZERO).await.unwrap_err();
        assert!(e.to_string().contains("incompleto"), "{e}");
        assert!(!destino.exists());
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn cancelar_apaga_o_parcial() {
        let d = dados();
        let (url, _) = servidor_de_arquivo(d, None).await;
        let pasta = pasta();
        let destino = pasta.join("m.bin");
        let cancelar = AtomicBool::new(true);
        let e = baixar(&reqwest::Client::new(), &url, &destino, &Esperado::default(), &sem_progresso(), &cancelar, Duration::ZERO).await.unwrap_err();
        assert!(e.to_string().contains("cancelado"));
        assert!(!destino.exists() && !caminho_parcial(&destino).exists());
        let _ = std::fs::remove_dir_all(pasta);
    }

    #[tokio::test]
    async fn erro_404_nao_repete() {
        let (url, ranges) = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/x", listener.local_addr().unwrap());
            let n = Arc::new(Mutex::new(Vec::<u8>::new()));
            let n2 = n.clone();
            tokio::spawn(async move {
                loop {
                    let (mut s, _) = listener.accept().await.unwrap();
                    n2.lock().unwrap().push(0u8);
                    let mut b = vec![0u8; 1024];
                    let _ = s.read(&mut b).await;
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                }
            });
            (url, n)
        };
        let pasta = pasta();
        let e = baixar(&reqwest::Client::new(), &url, &pasta.join("m.bin"), &Esperado::default(), &sem_progresso(), &AtomicBool::new(false), Duration::ZERO).await.unwrap_err();
        assert!(e.to_string().contains("404"));
        assert_eq!(ranges.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(pasta);
    }
}
