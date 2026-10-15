//! Teste com os programas e modelos reais (baixa vários GB). Não roda no `npm test`.
//!
//! Uso (PowerShell):
//!   $env:LOCAIS_RAIZ = "C:\\pasta\\vazia"; $env:LOCAIS_WAV = "C:\\fala.wav"
//!   cargo test -p busca_core --test real_local -- --ignored --nocapture

use busca_core::locais::Locais;
use busca_core::settings::Config;
use busca_core::summary::{OpenAi, Resumidor};
use busca_core::whisper::{ClienteWhisper, Transcritor};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn baixar_e_esperar(l: &Arc<Locais>, id: &str, aceleracao: &str) {
    let inicio = Instant::now();
    l.iniciar_download(id, aceleracao).unwrap();
    let mut ultimo = Instant::now() - Duration::from_secs(10);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let item = l.listar("cpu").as_array().unwrap().iter().find(|m| m["id"] == id).unwrap().clone();
        match item["estado"].as_str().unwrap() {
            "pronto" => break,
            "erro" => panic!("download falhou: {}", item["erro"]),
            _ if ultimo.elapsed() > Duration::from_secs(5) => {
                ultimo = Instant::now();
                println!("  {id}: {} {} / {}", item["fase"], item["baixado"], item["total"]);
            }
            _ => {}
        }
    }
    println!("{id} ({aceleracao}) pronto em {:.0}s", inicio.elapsed().as_secs_f64());
}

const TRANSCRICAO: &str = "[00:00:00] Outros: Bom dia, pessoal. Vamos falar do contrato com a empresa Alfa.
[00:00:12] Eu: Bom dia. O valor fechado foi de dez mil reais, com entrega em quinze de março.
[00:00:30] Outros: Combinado. Quem envia a proposta formal?
[00:00:36] Eu: Eu envio a proposta até sexta-feira.
[00:00:45] Outros: A Marina vai revisar o texto jurídico até quarta, antes de enviar.
[00:00:58] Outros: Ainda não sabemos se o cliente aceita pagamento parcelado. Isso fica em aberto.";

const MATERIAL: &str = "Pauta: contrato Alfa.\n\n=== DOCUMENTO: Tabela de preços ===\nPlano Ouro: R$ 10.000, entrega em 60 dias.\nPlano Prata: R$ 6.000, entrega em 30 dias.\nGarantia de todos os planos: 24 meses.";

#[tokio::test]
#[ignore]
async fn modelos_locais_de_verdade() {
    let raiz = std::path::PathBuf::from(std::env::var("LOCAIS_RAIZ").expect("defina LOCAIS_RAIZ"));
    let l = Locais::new(raiz, Arc::new(|_, _| {}));
    let mut cfg = Config { analise_local: true, transcricao_local: true, ..Default::default() };

    // ---- transcrição local
    baixar_e_esperar(&l, "whisper-base", "cpu").await; // download real, com hash
    baixar_e_esperar(&l, "whisper-small", "cpu").await; // arquivo copiado antes (mesmo hash do catálogo)
    let t = l.testar_whisper(&cfg).await.unwrap();
    println!("whisper-server pronto em {t:.1}s");
    if let Ok(wav) = std::env::var("LOCAIS_WAV") {
        let bytes = std::fs::read(wav).unwrap();
        let pcm: Vec<i16> = bytes[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let cliente = ClienteWhisper::new("http://127.0.0.1:8178", "pt");
        let inicio = Instant::now();
        let texto = cliente.transcrever(&pcm).await.unwrap();
        println!("TRANSCRIÇÃO ({:.1}s para {:.1}s de áudio): {texto}", inicio.elapsed().as_secs_f64(), pcm.len() as f64 / 16000.0);
        assert!(!texto.is_empty());
    }

    // ---- análise local, nas duas aceleracoes
    for acel in ["cpu", "vulkan"] {
        cfg.aceleracao = acel.into();
        baixar_e_esperar(&l, "qwen3-4b-q4", acel).await;
        let t = l.testar_llm(&cfg).await.unwrap();
        println!("[{acel}] llama-server pronto e respondeu em {t:.1}s");
        let url = l.garantir_llm(&cfg).await.unwrap();
        let local = OpenAi::local(url);

        let inicio = Instant::now();
        let r = local.resumir(TRANSCRICAO, MATERIAL, "", "local").await;
        println!("[{acel}] resumo em {:.1}s: {r:#?}", inicio.elapsed().as_secs_f64());
        let r = r.expect("o servidor local precisa aceitar o esquema JSON do resumo");
        assert!(!r.titulo.is_empty() && !r.resumo.is_empty());

        for p in ["Qual o prazo de entrega do contrato Alfa?", "Qual a garantia?", "O que eu devo responder se perguntarem quem envia a proposta?", "Qual o endereço da empresa Alfa?"] {
            let inicio = Instant::now();
            let resp = local.responder(p, MATERIAL, TRANSCRICAO, "", "local").await.unwrap();
            println!("[{acel}] P: {p}\n   R ({:.1}s): {resp}", inicio.elapsed().as_secs_f64());
        }
    }
}
