//! Passo 0: baixa do Scryfall o arquivo "Oracle Cards" (uma entrada por carta
//! única do Magic, ~35 mil cartas) e salva localmente em data/.
//!
//! O Scryfall entrega hoje um JSONL comprimido (`jsonl_download_uri`, .jsonl.gz);
//! versões antigas da API davam um JSON único (`download_uri`). Aceitamos os dois,
//! e o `build` lê qualquer um deles.
//!
//! Documentação: https://scryfall.com/docs/api/bulk-data

use serde::Deserialize;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::Path;

const BULK_INFO: &str = "https://api.scryfall.com/bulk-data/oracle-cards";
// O Scryfall exige User-Agent e Accept em toda requisição.
const USER_AGENT: &str = "magic_rag/0.3 (local card search prototype)";

#[derive(Deserialize)]
struct BulkInfo {
    jsonl_download_uri: Option<String>,
    download_uri: Option<String>,
    updated_at: String,
    compressed_size: Option<u64>,
    size: Option<u64>,
}

pub fn run(output: &str) -> Result<(), Box<dyn Error>> {
    let info: BulkInfo = ureq::get(BULK_INFO)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .call()?
        .body_mut()
        .read_json()?;

    let url = info
        .jsonl_download_uri
        .or(info.download_uri)
        .ok_or("o Scryfall não informou link de download")?;
    let size = info.compressed_size.or(info.size);
    let mb = size.map_or("?".into(), |s| format!("{:.0}", s as f64 / 1e6));
    println!("baixando Oracle Cards (atualizado em {}, ~{mb} MB)...", info.updated_at);

    if let Some(dir) = Path::new(output).parent() {
        fs::create_dir_all(dir)?;
    }
    // grava num .part e só renomeia no fim, para nunca deixar um arquivo pela metade
    let tmp = format!("{output}.part");
    let resp = ureq::get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "*/*")
        .call()?;
    let mut reader = resp.into_body().into_reader();
    let mut file = BufWriter::new(File::create(&tmp)?);
    let bytes = io::copy(&mut reader, &mut file)?;
    drop(file);
    fs::rename(&tmp, output)?;

    println!("{:.1} MB salvos em {output}", bytes as f64 / 1e6);
    Ok(())
}
