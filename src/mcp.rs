//! Servidor MCP (Model Context Protocol) sobre stdio: `magic_rag mcp`.
//!
//! Deixa um LLM (Claude Code, Claude Desktop, ou qualquer cliente MCP) usar o
//! MTG-RAG como ferramenta. As cartas e o índice BM25 são carregados uma vez
//! só, em segundo plano, então cada consulta é rápida.
//!
//! Protocolo: JSON-RPC 2.0, uma mensagem por linha no stdin/stdout.
//! Tudo que não é protocolo (logs) vai para o stderr.
//!
//! Ferramentas: search_cards, get_card, suggest_deck, validate_deck, list_tags.

use crate::card::Card;
use crate::search::{self, Index, Query, Sort};
use crate::{deck, tags};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::sync::OnceLock;

const SERVER_NAME: &str = "mtg-rag";
const DEFAULT_PROTOCOL: &str = "2025-06-18";

struct Db {
    cards: Vec<Card>,
    index: Index,
}

static DB: OnceLock<Result<Db, String>> = OnceLock::new();

fn db() -> Result<&'static Db, String> {
    DB.get_or_init(|| {
        let t = std::time::Instant::now();
        let cards = crate::load_cards().map_err(|e| e.to_string())?;
        let index = Index::new(&cards);
        eprintln!("[mtg-rag] {} cartas carregadas em {:.1}s", cards.len(), t.elapsed().as_secs_f64());
        Ok(Db { cards, index })
    })
    .as_ref()
    .map_err(|e| e.clone())
}

const INSTRUCTIONS: &str = "MTG-RAG: busca local sobre todas as cartas de Magic: The Gathering (dados do Scryfall). \
Use estas ferramentas em vez de confiar na memória para texto, legalidade, preço ou existência de cartas. \
Dicas: (1) o texto de busca funciona melhor com palavras do texto oracle em inglês (ex.: 'sacrifice a creature draw'); \
(2) combine com filtros e tags — veja list_tags; tags podem ser qualificadas, ex. 'theme:tokens'; \
(3) sempre rode validate_deck antes de apresentar um deck ao usuário.";

// ---------------------------------------------------------------- definição das ferramentas

fn str_array(desc: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": desc})
}

fn tools() -> Value {
    json!([
        {
            "name": "search_cards",
            "title": "Buscar cartas",
            "description": "Busca cartas de Magic por texto (BM25 com stemming sobre o texto oracle em inglês + rótulos em português) e/ou filtros exatos. \
Sem 'query', ordena por popularidade no EDHREC. Para achar cartas por função, prefira 'tags' (ex.: removal, ramp, card_draw, sacrifice_outlet, death_value) — a lista completa está em list_tags.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Texto livre. Melhor em inglês, com palavras do texto da carta (ex.: 'whenever creature dies draw card')."},
                    "colors": {"type": "string", "description": "Identidade de cor permitida, ex. 'B,G'. A carta só pode usar essas cores. 'C' = só incolores."},
                    "has_colors": {"type": "string", "description": "Cores que a identidade precisa conter, ex. 'U'."},
                    "cmc_min": {"type": "number"},
                    "cmc_max": {"type": "number"},
                    "types": str_array("Tipos aceitos (qualquer um deles): Creature, Instant, Sorcery, Artifact, Enchantment, Land, Planeswalker, Battle, Legendary."),
                    "subtype": {"type": "string", "description": "Subtipo exato: Elf, Zombie, Equipment, Aura, Vehicle..."},
                    "tribe": {"type": "string", "description": "Tipo de criatura que a carta é OU menciona no texto (sinergia tribal)."},
                    "rarity": {"type": "string", "enum": ["common", "uncommon", "rare", "mythic"]},
                    "name": {"type": "string", "description": "O nome contém este texto."},
                    "tags": str_array("Exige TODAS estas tags."),
                    "any_tags": str_array("Exige AO MENOS UMA destas tags."),
                    "not_tags": str_array("Exclui cartas com estas tags."),
                    "boost": str_array("Tags que só aumentam a nota (não filtram)."),
                    "format": {"type": "string", "description": "Legal no formato: commander, pauper, modern, standard, pioneer, legacy, vintage, brawl, oathbreaker, paupercommander..."},
                    "commander_only": {"type": "boolean", "description": "Só cartas que podem ser comandante."},
                    "instant_speed": {"type": "boolean", "description": "Só instantâneos e cartas com Flash."},
                    "price_max": {"type": "number", "description": "Preço máximo em dólares."},
                    "paper": {"type": "boolean", "description": "Esconde cartas só digitais (Alchemy) e cartas 'Un-'."},
                    "no_game_changers": {"type": "boolean", "description": "Esconde Game Changers (brackets de Commander)."},
                    "sort": {"type": "string", "enum": ["relevance", "popularity", "cmc", "price", "name", "newest"]},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Quantos resultados (padrão 10)."}
                }
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "get_card",
            "title": "Ficha de carta",
            "description": "Ficha completa de uma carta pelo nome (exato, sem diferenciar maiúsculas; se não achar, por trecho do nome): texto oracle, tipos, P/T, legalidade em todos os formatos, preço, rank no EDHREC, tags e link do Scryfall. Devolve até 5 cartas.",
            "inputSchema": {
                "type": "object",
                "properties": {"name": {"type": "string", "description": "Nome da carta, ex. 'Sol Ring' ou 'Jarad'."}},
                "required": ["name"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "suggest_deck",
            "title": "Sugerir deck de Commander",
            "description": "Dado um comandante, sugere cartas legais em Commander, dentro da identidade de cor, por seção: ramp, compra, remoção, board wipes, proteção, tutores, anulações (se azul), cada tema do comandante, maior sinergia com o texto dele e terrenos de correção de cor. Nota = sinergia (BM25 com o texto do comandante) + popularidade.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commander": {"type": "string", "description": "Nome do comandante."},
                    "per_section": {"type": "integer", "minimum": 1, "maximum": 30, "description": "Cartas por seção (padrão 8)."},
                    "budget": {"type": "number", "description": "Preço máximo por carta, em dólares."},
                    "no_game_changers": {"type": "boolean"},
                    "extra_tags": str_array("Seções extras por tag, ex. ['sacrifice_outlet', 'self_mill'].")
                },
                "required": ["commander"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "validate_deck",
            "title": "Validar deck",
            "description": "Confere uma lista de deck contra a base: cartas inexistentes (com sugestões de nome), legalidade no formato, limite de cópias (4, ou 1 nos formatos singleton), identidade de cor do comandante, contagem de cartas, Game Changers, preço total e curva de mana. Aceita linhas '4 Lightning Bolt', '4x Lightning Bolt', 'Lightning Bolt', exportação do Arena ('4 Lightning Bolt (M10) 146') e um cabeçalho 'Sideboard'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "decklist": {"type": "string", "description": "A lista, uma carta por linha."},
                    "format": {"type": "string", "description": "Formato: commander, pauper, modern, standard, legacy, brawl..."},
                    "commander": {"type": "string", "description": "Comandante (ou dois parceiros separados por ' + '). Fica fora da contagem da lista."}
                },
                "required": ["decklist", "format"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "list_tags",
            "title": "Listar tags",
            "description": "Lista todas as tags de classificação por categoria (function, trigger, target, theme), com o significado em português e quantas cartas têm cada uma. Use para descobrir quais tags passar ao search_cards.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        }
    ])
}

// ---------------------------------------------------------------- leitura de argumentos

fn s(a: &Value, k: &str) -> Option<String> {
    a.get(k).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(String::from)
}
fn n(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64)
}
fn b(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn list(a: &Value, k: &str) -> Vec<String> {
    match a.get(k) {
        Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(String::from).collect(),
        Some(Value::String(x)) => x.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        _ => vec![],
    }
}
fn colors(a: &Value, k: &str) -> Option<Vec<String>> {
    s(a, k).map(|c| {
        c.to_uppercase()
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .flat_map(|p| if p.len() > 1 && !p.contains(',') { p.chars().map(String::from).collect() } else { vec![p.to_string()] })
            .filter(|x| !x.is_empty() && x != "C")
            .collect()
    })
}

// ---------------------------------------------------------------- ferramentas

fn search_cards(db: &Db, a: &Value) -> Result<Value, String> {
    let mut q = Query {
        text: s(a, "query").unwrap_or_default(),
        colors: colors(a, "colors"),
        colors_include: colors(a, "has_colors").unwrap_or_default(),
        cmc_min: n(a, "cmc_min"),
        cmc_max: n(a, "cmc_max"),
        types: list(a, "types"),
        subtype: s(a, "subtype"),
        tribe: s(a, "tribe"),
        rarity: s(a, "rarity"),
        name: s(a, "name"),
        tags: list(a, "tags"),
        any_tags: list(a, "any_tags"),
        not_tags: list(a, "not_tags"),
        boost: list(a, "boost"),
        format: s(a, "format"),
        commander_only: b(a, "commander_only"),
        instant_speed: b(a, "instant_speed"),
        price_max: n(a, "price_max"),
        paper: b(a, "paper"),
        no_game_changers: b(a, "no_game_changers"),
        top: a.get("limit").and_then(Value::as_u64).unwrap_or(10).clamp(1, 50) as usize,
        sort: Sort::default(),
    };
    if let Some(so) = s(a, "sort") {
        q.sort = Sort::parse(&so)?;
    }
    crate::validate_tags(q.tags.iter().chain(&q.any_tags).chain(&q.not_tags).chain(&q.boost))?;
    let hits = search::search(&db.cards, &db.index, &q);
    Ok(json!({
        "count": hits.len(),
        "cards": hits.iter().map(|h| crate::card_json(h.card, q.has_text().then_some(h.score), false)).collect::<Vec<_>>(),
    }))
}

fn get_card(db: &Db, a: &Value) -> Result<Value, String> {
    let name = s(a, "name").ok_or("informe 'name'")?;
    let found = search::find_by_name(&db.cards, &name);
    if found.is_empty() {
        return Ok(json!({"found": 0, "cards": [], "suggestions": suggest_names(db, &name)}));
    }
    Ok(json!({
        "found": found.len(),
        "cards": found.iter().take(5).map(|c| crate::card_json(c, None, true)).collect::<Vec<_>>(),
    }))
}

/// Nomes parecidos para quando o nome não existe ("Tortex" -> "Tortured Existence").
fn suggest_names(db: &Db, name: &str) -> Vec<String> {
    let low = name.to_lowercase();
    let words: Vec<&str> = low.split_whitespace().filter(|w| w.len() >= 3).collect();
    let mut scored: Vec<(usize, u32, &str)> = db
        .cards
        .iter()
        .filter_map(|c| {
            let cn = c.name.to_lowercase();
            // palavras inteiras em comum + prefixo comum (pega apelidos como "tortex")
            let common = words.iter().filter(|w| cn.contains(*w)).count();
            let prefix = cn.chars().zip(low.chars()).take_while(|(x, y)| x == y).count();
            let score = common * 10 + if prefix >= 4 { prefix } else { 0 };
            (score > 0).then_some((score, c.edhrec_rank.unwrap_or(u32::MAX), c.name.as_str()))
        })
        .collect();
    scored.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
    scored.into_iter().take(5).map(|(_, _, n)| n.to_string()).collect()
}

fn suggest_deck(db: &Db, a: &Value) -> Result<Value, String> {
    let commander = s(a, "commander").ok_or("informe 'commander'")?;
    let o = deck::DeckOpts {
        per: a.get("per_section").and_then(Value::as_u64).unwrap_or(8).clamp(1, 30) as usize,
        budget: n(a, "budget"),
        no_game_changers: b(a, "no_game_changers"),
        extra_tags: list(a, "extra_tags"),
    };
    crate::validate_tags(o.extra_tags.iter())?;
    let plan = deck::build(&db.cards, &db.index, &commander, &o)?;
    Ok(json!({
        "commander": crate::card_json(plan.commander, None, true),
        "sections": plan.sections.iter().map(|sec| json!({
            "key": sec.key,
            "title": sec.title,
            "cards": sec.cards.iter().map(|(sc, c)| json!({
                "name": c.name, "mana_cost": c.mana_cost, "type_line": c.type_line,
                "price_usd": c.price_usd, "edhrec_rank": c.edhrec_rank, "score": (sc * 100.0).round() / 100.0,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "note": "Sugestões, não um deck pronto. Monte a lista e confira com validate_deck.",
    }))
}

/// Arredonda para centavos. O "+ 0.0" troca -0.0 (soma vazia de f64 em Rust) por 0.0.
fn money(x: f64) -> f64 {
    (x * 100.0).round() / 100.0 + 0.0
}

const SINGLETON: &[&str] = &["commander", "brawl", "standardbrawl", "oathbreaker", "paupercommander", "duel", "predh", "gladiator"];

fn validate_deck(db: &Db, a: &Value) -> Result<Value, String> {
    let text = s(a, "decklist").ok_or("informe 'decklist'")?;
    let format = s(a, "format").ok_or("informe 'format'")?.to_lowercase();
    let singleton = SINGLETON.contains(&format.as_str());
    let line_re = Regex::new(r"^(?:(\d+)\s*x?\s+)?(.+?)(?:\s+\([A-Za-z0-9]{2,6}\)(?:\s+\S+)?)?\s*$").unwrap();

    let find = |name: &str| -> Option<&Card> {
        let hits = search::find_by_name(&db.cards, name);
        hits.first().copied().filter(|c| {
            let l = name.to_lowercase();
            c.name.to_lowercase() == l || c.name.to_lowercase().split(" // ").any(|f| f == l)
        })
    };

    // comandante(s)
    let mut commanders: Vec<&Card> = vec![];
    let mut problems: Vec<String> = vec![];
    let mut warnings: Vec<String> = vec![];
    let mut unknown: Vec<Value> = vec![];
    if let Some(cmd) = s(a, "commander") {
        for part in cmd.split(" + ") {
            match find(part.trim()) {
                Some(c) => {
                    if !c.can_be_commander {
                        warnings.push(format!("{} normalmente não pode ser comandante", c.name));
                    }
                    commanders.push(c);
                }
                None => unknown.push(json!({"name": part.trim(), "suggestions": suggest_names(db, part.trim())})),
            }
        }
    }
    let identity: Option<Vec<String>> = (!commanders.is_empty()).then(|| {
        let mut v: Vec<String> = commanders.iter().flat_map(|c| c.color_identity.clone()).collect();
        v.sort();
        v.dedup();
        v
    });

    // lista
    let mut main: BTreeMap<String, (u32, &Card)> = BTreeMap::new();
    let mut side: BTreeMap<String, (u32, &Card)> = BTreeMap::new();
    let mut in_side = false;
    for raw in text.lines() {
        let line = raw.trim();
        let low = line.to_lowercase();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            if low.contains("sideboard") {
                in_side = true;
            }
            continue;
        }
        match low.trim_end_matches(':') {
            "sideboard" | "side" => { in_side = true; continue; }
            "deck" | "main" | "maindeck" | "main deck" | "commander" | "companion" => { in_side = false; continue; }
            _ => {}
        }
        let Some(cap) = line_re.captures(line) else { continue };
        let qty: u32 = cap.get(1).map_or(1, |m| m.as_str().parse().unwrap_or(1));
        let name = cap[2].trim();
        match find(name) {
            Some(c) => {
                let slot = if in_side { &mut side } else { &mut main };
                slot.entry(c.name.clone()).or_insert((0, c)).0 += qty;
            }
            None => unknown.push(json!({"name": name, "suggestions": suggest_names(db, name)})),
        }
    }

    // regras
    let max_copies = if singleton { 1 } else { 4 };
    let mut totals: BTreeMap<String, (u32, &Card)> = BTreeMap::new();
    for (k, (q, c)) in main.iter().chain(side.iter()) {
        totals.entry(k.clone()).or_insert((0, c)).0 += q;
    }
    for c in &commanders {
        totals.entry(c.name.clone()).or_insert((0, c)).0 += 1;
    }
    let mut illegal = vec![];
    let mut off_color = vec![];
    for (name, (q, c)) in &totals {
        if !c.is_legal(&format) {
            illegal.push(name.clone());
        }
        let unlimited = c.supertypes.iter().any(|t| t == "Basic") || c.oracle_text.to_lowercase().contains("a deck can have any number of cards named");
        if !unlimited && *q > max_copies && !c.oracle_text.to_lowercase().contains("a deck can have up to") {
            problems.push(format!("{name}: {q} cópias (máximo {max_copies} em {format})"));
        }
        if let Some(id) = &identity
            && !c.color_identity.iter().all(|x| id.contains(x)) {
                off_color.push(format!("{name} ({})", c.color_identity.join("")));
            }
    }
    if !illegal.is_empty() {
        problems.push(format!("não legais em {format}: {}", illegal.join(", ")));
    }
    if !off_color.is_empty() {
        problems.push(format!("fora da identidade de cor do comandante: {}", off_color.join(", ")));
    }

    let main_count: u32 = main.values().map(|(q, _)| q).sum();
    let side_count: u32 = side.values().map(|(q, _)| q).sum();
    if !commanders.is_empty() {
        let total = main_count + commanders.len() as u32;
        if total != 100 {
            problems.push(format!("{total} cartas contando o(s) comandante(s); Commander exige exatamente 100"));
        }
    } else if !singleton {
        if main_count < 60 {
            problems.push(format!("main deck com {main_count} cartas; o mínimo é 60"));
        }
        if side_count > 15 {
            problems.push(format!("sideboard com {side_count} cartas; o máximo é 15"));
        }
    }
    if !unknown.is_empty() {
        problems.push(format!("{} carta(s) não encontrada(s) — veja 'unknown'", unknown.len()));
    }

    let price = |m: &BTreeMap<String, (u32, &Card)>| -> f64 {
        m.values().filter(|(_, c)| !c.supertypes.iter().any(|t| t == "Basic")).map(|(q, c)| *q as f64 * c.price_usd.unwrap_or(0.0)).sum()
    };
    let no_price: Vec<&String> = totals.iter().filter(|(_, (_, c))| c.price_usd.is_none() && !c.supertypes.iter().any(|t| t == "Basic")).map(|(n, _)| n).collect();
    if !no_price.is_empty() {
        warnings.push(format!("sem preço no Scryfall (fora do total): {}", no_price.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let mut curve: BTreeMap<u32, u32> = BTreeMap::new();
    let mut lands = 0;
    for (q, c) in main.values() {
        if c.types.iter().any(|t| t == "Land") {
            lands += q;
        } else {
            *curve.entry(c.cmc as u32).or_default() += q;
        }
    }
    let game_changers: Vec<&String> = totals.iter().filter(|(_, (_, c))| c.game_changer).map(|(n, _)| n).collect();

    Ok(json!({
        "valid": problems.is_empty(),
        "format": format,
        "commanders": commanders.iter().map(|c| &c.name).collect::<Vec<_>>(),
        "color_identity": identity,
        "main_count": main_count,
        "sideboard_count": side_count,
        "lands": lands,
        "curve": curve,
        "game_changers": game_changers,
        "price_usd": {
            "main": money(price(&main)),
            "sideboard": money(price(&side)),
            "commanders": money(commanders.iter().map(|c| c.price_usd.unwrap_or(0.0)).sum::<f64>()),
        },
        "problems": problems,
        "warnings": warnings,
        "unknown": unknown,
    }))
}

fn list_tags(db: Option<&Db>) -> Value {
    let cats: Vec<Value> = tags::TAXONOMY
        .iter()
        .map(|(cat, list)| {
            json!({
                "category": cat,
                "tags": list.iter().map(|(t, label)| {
                    let count = db.map(|d| d.cards.iter().filter(|c| c.tags.category(cat).is_some_and(|s| s.contains(*t))).count());
                    json!({"tag": t, "meaning": label, "cards": count})
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({"usage": "Passe em search_cards.tags; use 'categoria:tag' quando o nome repete (ex. 'theme:lifegain').", "categories": cats})
}

fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    if name == "list_tags" {
        return Ok(list_tags(db().ok()));
    }
    let db = db().map_err(|e| format!("base de cartas indisponível: {e}"))?;
    match name {
        "search_cards" => search_cards(db, args),
        "get_card" => get_card(db, args),
        "suggest_deck" => suggest_deck(db, args),
        "validate_deck" => validate_deck(db, args),
        _ => Err(format!("ferramenta desconhecida: {name}")),
    }
}

// ---------------------------------------------------------------- protocolo

/// Trata uma mensagem JSON-RPC. Devolve a resposta, ou None para notificações.
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let id = id?; // sem id = notificação (ex.: notifications/initialized): não responde

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let pv = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(DEFAULT_PROTOCOL);
            Ok(json!({
                "protocolVersion": pv,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "title": "MTG-RAG", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            // erro da ferramenta vai no resultado (isError), para o LLM poder corrigir
            Ok(match call_tool(name, &args) {
                Ok(v) => json!({
                    "content": [{"type": "text", "text": serde_json::to_string(&v).unwrap()}],
                    "isError": false,
                }),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            })
        }
        "" => Err((-32600, "requisição inválida".into())),
        other => Err((-32601, format!("método não suportado: {other}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    })
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[mtg-rag] servidor MCP iniciado (stdio)");
    // carrega as cartas em segundo plano: o initialize responde na hora
    std::thread::spawn(|| {
        if let Err(e) = db() {
            eprintln!("[mtg-rag] aviso: {e}");
        }
    });
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(handle).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(msg) => handle(&msg),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("JSON inválido: {e}")}})),
        };
        if let Some(r) = reply {
            writeln!(out, "{}", serde_json::to_string(&r)?)?;
            out.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_protocol_and_lists_tools() {
        let r = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "mtg-rag");
        assert!(r["result"]["capabilities"]["tools"].is_object());

        let r = handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["search_cards", "get_card", "suggest_deck", "validate_deck", "list_tags"]);
        for t in r["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        }
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_error() {
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = handle(&json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(&json!({"jsonrpc": "2.0", "id": 10, "method": "ping"})).unwrap();
        assert_eq!(r["result"], json!({}));
    }

    #[test]
    fn money_never_shows_negative_zero() {
        let empty: f64 = std::iter::empty::<f64>().sum();
        assert_eq!(money(empty).to_string(), "0");
        assert_eq!(money(42.649), 42.65);
    }

    #[test]
    fn list_tags_works_without_data() {
        let v = list_tags(None);
        let cats = v["categories"].as_array().unwrap();
        assert_eq!(cats.len(), 4);
        assert!(cats[0]["tags"].as_array().unwrap().iter().any(|t| t["tag"] == "removal"));
    }

    #[test]
    fn color_argument_accepts_several_spellings() {
        for raw in ["B,G", "BG", "b, g", "B G"] {
            assert_eq!(colors(&json!({"c": raw}), "c").unwrap(), vec!["B", "G"], "{raw}");
        }
        assert!(colors(&json!({"c": "C"}), "c").unwrap().is_empty());
    }
}
