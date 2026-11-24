//! Passo 1 do RAG: bulk do Scryfall -> data/cards.json
//! (metadados limpos + tags por categoria + texto de busca).

use crate::card::{Card, RawCard, RawFace};
use crate::tags::{self, Classifier, Input};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read};
use std::time::Instant;

/// Layouts que não são cartas jogáveis num deck.
const SKIP_LAYOUTS: &[&str] = &["token", "double_faced_token", "emblem", "art_series"];
const SUPERTYPES: &[&str] = &["Legendary", "Basic", "Snow", "World", "Ongoing", "Host", "Elite"];

/// "Legendary Creature — Elf Druid" -> (supertipos, tipos, subtipos)
fn parse_type_line(tl: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut sup, mut ty, mut sub) = (vec![], vec![], vec![]);
    for face in tl.split(" // ") {
        let (main, rest) = face.split_once(" — ").unwrap_or((face, ""));
        for w in main.split_whitespace() {
            let v = if SUPERTYPES.contains(&w) { &mut sup } else { &mut ty };
            if !v.iter().any(|x| x == w) {
                v.push(w.to_string());
            }
        }
        for w in rest.split_whitespace() {
            if !sub.iter().any(|x| x == w) {
                sub.push(w.to_string());
            }
        }
    }
    (sup, ty, sub)
}

/// Subtipos de criatura do dataset, para detectar sinergia tribal.
/// Tira cartas "Un-" (subtipos de piada) e subtipos que também existem fora de
/// criaturas (Equipment, Aura, Forest, Saga, Food...).
fn creature_types(raw: &[RawCard]) -> BTreeSet<String> {
    let (mut creature, mut other) = (BTreeSet::new(), BTreeSet::new());
    for c in raw.iter().filter(|c| c.set_type != "funny") {
        let lines = c.type_line.iter().map(String::as_str).chain(c.card_faces.iter().flatten().map(|f| f.type_line.as_str()));
        for tl in lines {
            for face in tl.split(" // ") {
                let Some((main, sub)) = face.split_once(" — ") else { continue };
                let words = sub.split_whitespace().map(String::from);
                if main.contains("Creature") {
                    creature.extend(words);
                } else if !main.contains("Kindred") && !main.contains("Tribal") {
                    other.extend(words);
                }
            }
        }
    }
    creature.difference(&other).cloned().collect()
}

fn price(s: &Option<String>) -> Option<f64> {
    s.as_deref().and_then(|p| p.parse().ok())
}

/// Faces únicas (cartas "reversible" repetem a mesma face dos dois lados).
fn unique_faces(c: &RawCard) -> Vec<&RawFace> {
    let mut seen = BTreeSet::new();
    c.card_faces.iter().flatten().filter(|f| seen.insert(f.name.as_str())).collect()
}

/// Lê o bulk do Scryfall: .gz ou não, JSON único (`[...]`) ou JSONL (uma carta por linha).
fn read_raw(input: &str) -> Result<Vec<RawCard>, Box<dyn Error>> {
    let mut bytes = vec![];
    File::open(input)
        .map_err(|e| format!("{input}: {e} (rode antes: magic_rag fetch)"))?
        .read_to_end(&mut bytes)?;
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = vec![];
        flate2::read::MultiGzDecoder::new(&bytes[..]).read_to_end(&mut out)?;
        bytes = out;
    }
    let first = bytes.iter().find(|b| !b.is_ascii_whitespace()).copied();
    if first == Some(b'[') {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let mut cards = vec![];
    for (i, line) in BufReader::new(&bytes[..]).lines().enumerate() {
        let line = line?;
        if !line.trim().is_empty() {
            cards.push(serde_json::from_str(&line).map_err(|e| format!("linha {}: {e}", i + 1))?);
        }
    }
    Ok(cards)
}

pub fn run(input: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let t0 = Instant::now();
    let raw = read_raw(input)?;
    let total = raw.len();
    let raw: Vec<RawCard> = raw.into_iter().filter(|c| !SKIP_LAYOUTS.contains(&c.layout.as_str())).collect();
    println!("{total} entradas lidas, {} cartas jogáveis ({:.1}s)", raw.len(), t0.elapsed().as_secs_f64());

    let classifier = Classifier::new(&creature_types(&raw));

    let mut cards = Vec::with_capacity(raw.len());
    for c in raw {
        let faces = unique_faces(&c);
        let multi = faces.len() > 1;

        // texto oracle: carta simples ou faces juntadas
        let top_text = c.oracle_text.clone().unwrap_or_default();
        let (display_text, tag_text) = if multi && top_text.is_empty() {
            let display = faces
                .iter()
                .map(|f| {
                    let cost = if f.mana_cost.is_empty() { String::new() } else { format!(" {}", f.mana_cost) };
                    format!("[{}{} — {}]\n{}", f.name, cost, f.type_line, f.oracle_text)
                })
                .collect::<Vec<_>>()
                .join("\n//\n");
            let tag = faces.iter().map(|f| f.oracle_text.as_str()).collect::<Vec<_>>().join("\n");
            (display, tag)
        } else {
            (top_text.clone(), top_text)
        };

        let type_line = c.type_line.clone().unwrap_or_else(|| {
            faces.iter().map(|f| f.type_line.as_str()).collect::<Vec<_>>().join(" // ")
        });
        let mana_cost = c.mana_cost.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| {
            faces.iter().map(|f| f.mana_cost.as_str()).filter(|m| !m.is_empty()).collect::<Vec<_>>().join(" // ")
        });
        let front = faces.first();
        let pick = |top: &Option<String>, face: fn(&RawFace) -> &Option<String>| {
            top.clone().or_else(|| front.and_then(|f| face(f).clone()))
        };
        let power = pick(&c.power, |f| &f.power);
        let toughness = pick(&c.toughness, |f| &f.toughness);
        let loyalty = pick(&c.loyalty, |f| &f.loyalty);
        let defense = pick(&c.defense, |f| &f.defense);

        let (supertypes, types, subtypes) = parse_type_line(&type_line);
        let produced_mana = c.produced_mana.clone().unwrap_or_default();

        let mut names = vec![c.name.as_str()];
        names.extend(faces.iter().map(|f| f.name.as_str()));
        let (mut tags, tribes) = classifier.classify(&Input {
            names,
            text: &tag_text,
            types: &types,
            subtypes: &subtypes,
            keywords: &c.keywords,
            produced_mana: &produced_mana,
        });
        tags::apply_manual(&c.name, &mut tags);

        let front_type = type_line.split(" // ").next().unwrap_or("");
        let can_be_commander = (front_type.contains("Legendary") && front_type.contains("Creature"))
            || tag_text.contains("can be your commander");
        let instant_speed = types.iter().any(|t| t == "Instant") || c.keywords.iter().any(|k| k == "Flash");

        let mut legal_formats: Vec<String> = c
            .legalities
            .iter()
            .filter(|(_, v)| *v == "legal" || *v == "restricted")
            .map(|(k, _)| k.clone())
            .collect();
        legal_formats.sort();
        let mut restricted_formats: Vec<String> =
            c.legalities.iter().filter(|(_, v)| *v == "restricted").map(|(k, _)| k.clone()).collect();
        restricted_formats.sort();

        let colors = c.colors.clone().unwrap_or_else(|| {
            // cartas de várias faces só têm cores por face; a identidade cobre o resto
            c.color_identity.clone()
        });

        // texto de busca: inglês original + rótulos em português
        let cores = if c.color_identity.is_empty() {
            "incolor".to_string()
        } else {
            c.color_identity.iter().map(|x| tags::color_pt(x)).collect::<Vec<_>>().join(", ")
        };
        let tipos_pt: Vec<&str> = supertypes.iter().chain(&types).filter_map(|t| tags::type_pt(t)).collect();
        let tag_words: Vec<&str> =
            tags.function.iter().chain(&tags.trigger).chain(&tags.theme).filter_map(|t| tags::pt_label(t)).collect();
        let search_text = format!(
            "{}. {}. {}. Custo {} (cmc {}). Cores: {}. {} Keywords: {}. Tribos: {}. Tags: {}.",
            c.name,
            type_line,
            tipos_pt.join(", "),
            mana_cost,
            c.cmc as i64,
            cores,
            classifier.strip_reminder(&display_text),
            c.keywords.join(", "),
            tribes.join(", "),
            tag_words.join("; "),
        );

        let image = c
            .image_uris
            .as_ref()
            .or_else(|| front.and_then(|f| f.image_uris.as_ref()))
            .and_then(|m| m.get("normal").cloned());

        cards.push(Card {
            oracle_id: c.oracle_id.clone().or_else(|| front.and_then(|f| f.oracle_id.clone())).unwrap_or_default(),
            name: c.name,
            layout: c.layout,
            mana_cost,
            cmc: c.cmc,
            type_line,
            supertypes,
            types,
            subtypes,
            colors,
            color_identity: c.color_identity,
            keywords: c.keywords,
            power,
            toughness,
            loyalty,
            defense,
            produced_mana,
            rarity: c.rarity,
            set: c.set,
            set_name: c.set_name,
            released_at: c.released_at,
            legal_formats,
            restricted_formats,
            edhrec_rank: c.edhrec_rank,
            game_changer: c.game_changer,
            reserved: c.reserved,
            digital_only: !c.games.iter().any(|g| g == "paper"),
            funny: c.set_type == "funny",
            can_be_commander,
            instant_speed,
            price_usd: price(&c.prices.usd),
            price_eur: price(&c.prices.eur),
            oracle_text: display_text,
            tags,
            tribes,
            scryfall_uri: c.scryfall_uri,
            image,
            search_text,
        });
    }

    // ordem estável: mais populares primeiro, depois por nome
    cards.sort_by(|a, b| {
        a.edhrec_rank.unwrap_or(u32::MAX).cmp(&b.edhrec_rank.unwrap_or(u32::MAX)).then_with(|| a.name.cmp(&b.name))
    });

    if let Some(dir) = std::path::Path::new(output).parent() {
        fs::create_dir_all(dir)?;
    }
    serde_json::to_writer(BufWriter::new(File::create(output)?), &cards)?;

    // resumo
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for c in &cards {
        for (cat, set) in [("function", &c.tags.function), ("trigger", &c.tags.trigger), ("target", &c.tags.target), ("theme", &c.tags.theme)] {
            for t in set {
                *count.entry(format!("{cat}:{t}")).or_default() += 1;
            }
        }
    }
    let mut top: Vec<_> = count.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    println!(
        "{} cartas salvas em {output} ({:.1}s). Tags mais comuns: {}",
        cards.len(),
        t0.elapsed().as_secs_f64(),
        top.iter().take(8).map(|(t, n)| format!("{t}={n}")).collect::<Vec<_>>().join(", ")
    );
    Ok(())
}
