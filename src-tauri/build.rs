use std::path::Path;

/// Lê `chave=valor` de um arquivo .env (ignora linhas em branco e comentários).
fn ler_env(caminho: &Path) -> Vec<(String, String)> {
    let Ok(texto) = std::fs::read_to_string(caminho) else { return vec![] };
    texto
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()))
        })
        .collect()
}

fn main() {
    // O .env fica só na sua máquina: os valores são embutidos no programa na hora de gerar o instalador.
    let env = Path::new("..").join(".env");
    println!("cargo:rerun-if-changed={}", env.display());
    println!("cargo:rerun-if-env-changed=GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=GOOGLE_CLIENT_SECRET");
    let arquivo = ler_env(&env);
    for chave in ["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"] {
        let valor = std::env::var(chave).ok().filter(|v| !v.is_empty()).or_else(|| arquivo.iter().find(|(k, _)| k == chave).map(|(_, v)| v.clone()));
        if let Some(v) = valor.filter(|v| !v.is_empty()) {
            println!("cargo:rustc-env={chave}={v}");
        }
    }
    tauri_build::build()
}
