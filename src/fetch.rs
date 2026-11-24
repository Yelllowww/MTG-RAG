//! Passo 0: baixa as fontes oficiais para data/.
//!
//! * Scryfall "Oracle Cards": uma entrada por carta única (~36 mil cartas).
//! * Scryfall "Rulings": esclarecimentos oficiais por carta (Wizards + Scryfall).
//! * Comprehensive Rules da Wizards, em TXT (para busca) e PDF (para o usuário
//!   conferir uma regra pelo número).
//!
//! O Scryfall entrega hoje JSONL comprimido (`jsonl_download_uri`, .jsonl.gz);
//! versões antigas da API davam um JSON único (`download_uri`). Aceitamos os dois.
//! O link das regras muda a cada atualização (o nome do arquivo tem a data), então
//! ele é lido da página https://magic.wizards.com/en/rules.
//!
//! Documentação: https://scryfall.com/docs/api/bulk-data

use regex::Regex;
use serde::Deserialize;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::Path;

// O Scryfall exige User-Agent e Accept em toda requisição.
const USER_AGENT: &str = concat!("magic_rag/", env!("CARGO_PKG_VERSION"), " (local card search prototype)");
const RULES_PAGE: &str = "https://magic.wizards.com/en/rules";

pub const CARDS_RAW: &str = "scryfall-oracle-cards.jsonl.gz";
pub const RULINGS_RAW: &str = "scryfall-rulings.jsonl.gz";
pub const RULES_TXT: &str = "comprehensive-rules.txt";
pub const RULES_PDF: &str = "comprehensive-rules.pdf";

#[derive(Deserialize)]
struct BulkInfo {
    jsonl_download_uri: Option<String>,
    download_uri: Option<String>,
    updated_at: String,
    compressed_size: Option<u64>,
    size: Option<u64>,
}

/// Baixa tudo para a pasta `dir`.
pub fn run_all(dir: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(dir)?;
    bulk("oracle-cards", "cartas (Oracle Cards)", &dir.join(CARDS_RAW))?;
    bulk("rulings", "rulings das cartas", &dir.join(RULINGS_RAW))?;
    rules(dir)?;
    Ok(())
}

/// Baixa um arquivo "bulk data" do Scryfall.
pub fn bulk(kind: &str, label: &str, output: &Path) -> Result<(), Box<dyn Error>> {
    let info: BulkInfo = ureq::get(&format!("https://api.scryfall.com/bulk-data/{kind}"))
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .call()?
        .body_mut()
        .read_json()?;
    let url = info.jsonl_download_uri.or(info.download_uri).ok_or("o Scryfall não informou link de download")?;
    let mb = info.compressed_size.or(info.size).map_or("?".into(), |s| format!("{:.0}", s as f64 / 1e6));
    println!("baixando {label} (atualizado em {}, ~{mb} MB)...", &info.updated_at[..10.min(info.updated_at.len())]);
    download(&url, output)
}

/// Acha os links atuais do TXT e do PDF das Comprehensive Rules e baixa os dois.
fn rules(dir: &Path) -> Result<(), Box<dyn Error>> {
    let page = ureq::get(RULES_PAGE).header("User-Agent", USER_AGENT).call()?.body_mut().read_to_string()?;
    let (txt, pdf) = rules_links(&page).ok_or("não achei os links das Comprehensive Rules em magic.wizards.com/en/rules")?;
    println!("baixando Comprehensive Rules ({})...", txt.rsplit('/').next().unwrap_or(&txt).replace("%20", " "));
    download(&txt, &dir.join(RULES_TXT))?;
    download(&pdf, &dir.join(RULES_PDF))?;
    Ok(())
}

/// Links (TXT, PDF) das regras na página da Wizards. O nome tem espaço ("MagicCompRules 20261106.txt").
pub fn rules_links(html: &str) -> Option<(String, String)> {
    let re = Regex::new(r#"https://media\.wizards\.com/[^"'<>]*?MagicCompRules[^"'<>]*?\.(txt|pdf)"#).unwrap();
    let mut txt = None;
    let mut pdf = None;
    for m in re.find_iter(html) {
        let url = m.as_str().replace(' ', "%20");
        if url.ends_with(".txt") && txt.is_none() {
            txt = Some(url);
        } else if url.ends_with(".pdf") && pdf.is_none() {
            pdf = Some(url);
        }
    }
    Some((txt?, pdf?))
}

/// Baixa para um .part e só renomeia no fim, para nunca deixar um arquivo pela metade.
fn download(url: &str, output: &Path) -> Result<(), Box<dyn Error>> {
    let tmp = output.with_extension("part");
    let resp = ureq::get(url).header("User-Agent", USER_AGENT).header("Accept", "*/*").call()?;
    let mut reader = resp.into_body().into_reader();
    let mut file = BufWriter::new(File::create(&tmp)?);
    let bytes = io::copy(&mut reader, &mut file)?;
    drop(file);
    fs::rename(&tmp, output)?;
    println!("  {:.1} MB salvos em {}", bytes as f64 / 1e6, output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_rules_links_with_spaces() {
        let html = r#"<a href="https://media.wizards.com/2026/downloads/MagicCompRules 20261106.pdf">PDF</a>
            <a href="https://media.wizards.com/2026/downloads/MagicCompRules 20261106.txt">TXT</a>
            <a href="https://media.wizards.com/2026/downloads/MagicCompRules 20261106.docx">DOCX</a>"#;
        let (txt, pdf) = super::rules_links(html).unwrap();
        assert_eq!(txt, "https://media.wizards.com/2026/downloads/MagicCompRules%2020261106.txt");
        assert!(pdf.ends_with("MagicCompRules%2020261106.pdf"));
        assert!(super::rules_links("<html>nada</html>").is_none());
    }
}
