//! Material de apoio: texto colado e documentos anexados (Google Drive ou arquivo do computador).
//! Também escolhe os trechos mais relevantes quando o material não cabe numa pergunta.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Limite de texto de um documento depois de extraído.
const LIMITE_DOCUMENTO: usize = 1_500_000;
/// Limite do arquivo bruto aceito.
pub const LIMITE_ARQUIVO: usize = 25 * 1024 * 1024;
const MARCA: &str = "=== DOCUMENTO: ";
const TAM_TRECHO: usize = 1200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Documento {
    pub id: String,
    pub nome: String,
    /// "drive" ou "arquivo".
    pub origem: String,
    pub texto: String,
}

/// Junta o texto colado e os documentos num só texto, com uma marca antes de cada documento.
/// Sem documentos, devolve o texto colado sem alteração.
pub fn compor(texto: &str, docs: &[Documento]) -> String {
    let mut s = texto.to_string();
    for d in docs {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&format!("{MARCA}{} ===\n{}", d.nome.replace('\n', " "), d.texto));
    }
    s
}

fn dobrar(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        outro => outro,
    }
}

const SEM_VALOR: &[&str] = &[
    "para", "com", "que", "uma", "uns", "umas", "como", "dos", "das", "por", "nao", "mais", "sobre", "qual", "quais", "isso",
    "esse", "essa", "esses", "essas", "este", "esta", "isto", "ser", "sao", "foi", "tem", "seu", "sua", "seus", "suas", "ele",
    "ela", "eles", "elas", "nos", "nas", "aos", "das", "dele", "dela", "quando", "onde", "quem", "porque", "pelo", "pela",
    "entre", "ate", "mas", "ainda", "tambem", "muito", "pode", "podem", "deve", "ter", "fazer", "ha", "ja", "num", "numa",
];

fn termos(texto: &str) -> Vec<String> {
    texto
        .to_lowercase()
        .chars()
        .map(dobrar)
        .collect::<String>()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 3 && !SEM_VALOR.contains(t))
        .map(str::to_string)
        .collect()
}

fn cortar(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Divide em trechos de cerca de `TAM_TRECHO` bytes, respeitando linhas e, se preciso, cortando a linha.
fn dividir(texto: &str) -> Vec<String> {
    let mut trechos = Vec::new();
    let mut atual = String::new();
    for linha in texto.lines() {
        let mut resto = linha;
        while resto.len() > TAM_TRECHO + TAM_TRECHO / 4 {
            let parte = cortar(resto, TAM_TRECHO);
            if !atual.is_empty() {
                trechos.push(std::mem::take(&mut atual));
            }
            trechos.push(parte.to_string());
            resto = &resto[parte.len()..];
        }
        if !atual.is_empty() && atual.len() + resto.len() + 1 > TAM_TRECHO {
            trechos.push(std::mem::take(&mut atual));
        }
        if !atual.is_empty() {
            atual.push('\n');
        }
        atual.push_str(resto);
    }
    if !atual.trim().is_empty() {
        trechos.push(atual);
    }
    trechos.retain(|t| !t.trim().is_empty());
    trechos
}

/// Separa o texto composto em (texto colado, [(nome, texto)]).
fn separar(material: &str) -> (String, Vec<(String, String)>) {
    let mut partes = material.split(MARCA);
    let intro = partes.next().unwrap_or("").trim().to_string();
    let docs = partes
        .map(|p| {
            let (nome, resto) = p.split_once(" ===\n").unwrap_or((p, ""));
            (nome.to_string(), resto.to_string())
        })
        .collect();
    (intro, docs)
}

/// Se o material cabe em `orcamento` bytes, devolve igual. Senão, mantém o texto colado (até um terço do
/// orçamento) e escolhe dos documentos os trechos que mais combinam com a `consulta` (pergunta e fala recente).
pub fn reduzir(material: &str, consulta: &str, orcamento: usize) -> String {
    if material.len() <= orcamento {
        return material.to_string();
    }
    let (intro, docs) = separar(material);
    let intro = cortar(&intro, orcamento / 3).to_string();
    let restante = orcamento.saturating_sub(intro.len());

    struct Trecho {
        doc: usize,
        pos: usize,
        texto: String,
        termos: Vec<String>,
        nota: f64,
    }
    let mut todos: Vec<Trecho> = vec![];
    for (i, (_, texto)) in docs.iter().enumerate() {
        for (pos, t) in dividir(texto).into_iter().enumerate() {
            let tt = termos(&t);
            todos.push(Trecho { doc: i, pos, texto: t, termos: tt, nota: 0.0 });
        }
    }
    let n = todos.len().max(1) as f64;
    let procurados = termos(consulta);
    for termo in procurados.iter().collect::<std::collections::BTreeSet<_>>() {
        let df = todos.iter().filter(|t| t.termos.contains(termo)).count() as f64;
        if df == 0.0 {
            continue;
        }
        let idf = (1.0 + n / df).ln();
        for t in todos.iter_mut() {
            let tf = t.termos.iter().filter(|x| *x == termo).count() as f64;
            if tf > 0.0 {
                t.nota += (1.0 + tf.ln()) * idf;
            }
        }
    }

    let mut ordem: Vec<usize> = (0..todos.len()).collect();
    ordem.sort_by(|&a, &b| {
        todos[b].nota.partial_cmp(&todos[a].nota).unwrap_or(std::cmp::Ordering::Equal)
            .then(todos[a].pos.cmp(&todos[b].pos))
            .then(todos[a].doc.cmp(&todos[b].doc))
    });
    let mut usado = 0;
    let mut escolhidos: Vec<usize> = vec![];
    for i in ordem {
        let custo = todos[i].texto.len() + 8;
        if usado + custo <= restante {
            usado += custo;
            escolhidos.push(i);
        }
    }
    escolhidos.sort_by_key(|&i| (todos[i].doc, todos[i].pos));

    let mut saida = intro;
    for (i, (nome, _)) in docs.iter().enumerate() {
        let meus: Vec<&Trecho> = escolhidos.iter().map(|&k| &todos[k]).filter(|t| t.doc == i).collect();
        if meus.is_empty() {
            continue;
        }
        if !saida.is_empty() {
            saida.push_str("\n\n");
        }
        saida.push_str(&format!("{MARCA}{nome} (só os trechos mais relevantes) ===\n"));
        saida.push_str(&meus.iter().map(|t| t.texto.as_str()).collect::<Vec<_>>().join("\n[...]\n"));
    }
    saida
}

fn decodificar_entidades(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Remove marcações `<...>` de HTML ou XML.
fn tirar_tags(s: &str) -> String {
    let mut fora = String::with_capacity(s.len());
    let mut dentro = false;
    for c in s.chars() {
        match c {
            '<' => dentro = true,
            '>' if dentro => {
                dentro = false;
                fora.push(' ');
            }
            _ if !dentro => fora.push(c),
            _ => {}
        }
    }
    decodificar_entidades(&fora)
}

fn texto_docx(bytes: &[u8]) -> Result<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| anyhow!("O arquivo .docx está corrompido."))?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .map_err(|_| anyhow!("O arquivo não parece um .docx válido."))?
        .read_to_string(&mut xml)?;
    let xml = xml
        .replace("</w:p>", "\n")
        .replace("<w:br/>", "\n")
        .replace("<w:tab/>", "\t");
    let mut texto = String::new();
    let mut dentro = false;
    for c in xml.chars() {
        match c {
            '<' => dentro = true,
            '>' if dentro => dentro = false,
            _ if !dentro => texto.push(c),
            _ => {}
        }
    }
    Ok(decodificar_entidades(&texto))
}

fn texto_pdf(bytes: &[u8]) -> Result<String> {
    let bytes = bytes.to_vec();
    std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&bytes))
        .map_err(|_| anyhow!("Não consegui ler este PDF."))?
        .map_err(|e| anyhow!("Não consegui ler este PDF: {e}"))
}

/// Extrai o texto de um arquivo. Aceita TXT, MD, CSV, JSON, HTML, DOCX e PDF (com texto, sem OCR).
pub fn extrair_texto(nome: &str, mime: &str, bytes: &[u8]) -> Result<String> {
    if bytes.len() > LIMITE_ARQUIVO {
        return Err(anyhow!("{nome} passa de {} MB.", LIMITE_ARQUIVO / 1024 / 1024));
    }
    let ext = nome.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let texto = match ext.as_str() {
        "pdf" => texto_pdf(bytes)?,
        "docx" => texto_docx(bytes)?,
        "doc" => return Err(anyhow!("O formato .doc antigo não é suportado. Salve {nome} como .docx.")),
        "html" | "htm" | "xml" => tirar_tags(&String::from_utf8_lossy(bytes)),
        "txt" | "md" | "csv" | "tsv" | "json" | "log" => String::from_utf8_lossy(bytes).into_owned(),
        _ if mime == "application/pdf" => texto_pdf(bytes)?,
        _ if mime.starts_with("text/") => String::from_utf8_lossy(bytes).into_owned(),
        _ => return Err(anyhow!("Tipo de arquivo sem suporte: {nome}. Use TXT, MD, CSV, DOCX ou PDF.")),
    };
    let texto = texto.replace("\r\n", "\n").trim().to_string();
    if texto.is_empty() {
        return Err(anyhow!("Não encontrei texto em {nome}. PDF escaneado (imagem) precisa de OCR, que o app não faz."));
    }
    Ok(cortar(&texto, LIMITE_DOCUMENTO).to_string())
}

#[cfg(test)]
mod testes {
    use super::*;
    use std::io::Write;

    fn doc(nome: &str, texto: &str) -> Documento {
        Documento { id: nome.into(), nome: nome.into(), origem: "arquivo".into(), texto: texto.into() }
    }

    #[test]
    fn sem_documentos_o_texto_fica_igual() {
        assert_eq!(compor("Pauta", &[]), "Pauta");
        assert_eq!(compor("", &[]), "");
    }

    #[test]
    fn documentos_vem_com_marca_e_nome() {
        let m = compor("Pauta", &[doc("Preços.pdf", "Plano Ouro: R$ 100")]);
        assert_eq!(m, "Pauta\n\n=== DOCUMENTO: Preços.pdf ===\nPlano Ouro: R$ 100");
        assert_eq!(compor("", &[doc("a", "x")]), "=== DOCUMENTO: a ===\nx");
    }

    #[test]
    fn material_pequeno_nao_muda() {
        let m = compor("Pauta", &[doc("a", "texto")]);
        assert_eq!(reduzir(&m, "qualquer", 10_000), m);
    }

    #[test]
    fn material_grande_mantem_so_o_que_responde_a_pergunta() {
        let enchimento = "Linha sem importância sobre outro assunto qualquer. ".repeat(40);
        let mut texto = String::new();
        for i in 0..30 {
            texto += &format!("{enchimento}\n");
            if i == 20 {
                texto += "O prazo de entrega do contrato Alfa é 15 de março.\n";
            }
        }
        let m = compor("Pauta curta", &[doc("Contratos", &texto), doc("Outro", &enchimento)]);
        assert!(m.len() > 20_000);
        let r = reduzir(&m, "Qual o prazo de entrega do contrato Alfa?", 8_000);
        assert!(r.len() <= 8_000 + 400, "tamanho {}", r.len());
        assert!(r.starts_with("Pauta curta"));
        assert!(r.contains("15 de março"), "o trecho certo precisa estar na seleção");
        assert!(r.contains("só os trechos mais relevantes"));
    }

    #[test]
    fn acento_nao_atrapalha_a_busca_de_trechos() {
        let enchimento = "texto neutro de enchimento para ocupar espaço. ".repeat(60);
        let m = compor("", &[doc("D", &format!("{enchimento}\nA reunião de negociação define o preço.\n{enchimento}\n{enchimento}"))]);
        let r = reduzir(&m, "reuniao negociacao preco", 4_000);
        assert!(r.contains("negociação"));
    }

    #[test]
    fn extrai_texto_simples_html_e_recusa_o_que_nao_conhece() {
        assert_eq!(extrair_texto("a.txt", "", "Olá\r\nmundo ".as_bytes()).unwrap(), "Olá\nmundo");
        assert_eq!(extrair_texto("a.html", "", b"<p>Oi &amp; tchau</p>").unwrap().trim(), "Oi & tchau");
        assert!(extrair_texto("a.png", "image/png", b"x").unwrap_err().to_string().contains("sem suporte"));
        assert!(extrair_texto("a.doc", "", b"x").unwrap_err().to_string().contains(".docx"));
        assert!(extrair_texto("a.txt", "", b"   ").unwrap_err().to_string().contains("Não encontrei texto"));
    }

    #[test]
    fn extrai_texto_de_docx() {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(br#"<w:document><w:body><w:p><w:r><w:t>Primeiro &amp; segundo</w:t></w:r></w:p><w:p><w:r><w:t>Linha dois</w:t></w:r></w:p></w:body></w:document>"#).unwrap();
            z.finish().unwrap();
        }
        assert_eq!(extrair_texto("contrato.docx", "", &buf).unwrap(), "Primeiro & segundo\nLinha dois");
        assert!(extrair_texto("ruim.docx", "", b"nao e zip").unwrap_err().to_string().contains("corrompido"));
    }

    #[test]
    fn pdf_invalido_da_erro_sem_derrubar_o_app() {
        assert!(extrair_texto("a.pdf", "application/pdf", b"isto nao e pdf").is_err());
    }
}
