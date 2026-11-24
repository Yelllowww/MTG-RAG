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
use crate::rules::{self, RuleHit, RulesDb};
use crate::{deck, fetch, tags};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, Write};
use std::sync::OnceLock;

const SERVER_NAME: &str = "mtg-rag";
const DEFAULT_PROTOCOL: &str = "2025-06-18";

struct Db {
    cards: Vec<Card>,
    index: Index,
    /// nome normalizado (e de cada face) -> posição em `cards`: busca exata O(1)
    by_name: HashMap<String, usize>,
}

impl Db {
    /// Carta com este nome exato (ignora maiúsculas, acentos, apóstrofos; aceita nome de uma face).
    fn exact(&self, name: &str) -> Option<&Card> {
        self.by_name.get(&search::norm_name(name)).map(|&i| &self.cards[i])
    }
}

static DB: OnceLock<Result<Db, String>> = OnceLock::new();
static RULES: OnceLock<Result<RulesDb, String>> = OnceLock::new();

fn rules_db() -> Result<&'static RulesDb, String> {
    RULES
        .get_or_init(|| {
            let t = std::time::Instant::now();
            let r = RulesDb::load(&crate::data_dir())?;
            eprintln!("[mtg-rag] {} regras e rulings de {} cartas carregados em {:.1}s", r.doc.rules.len(), r.rulings.len(), t.elapsed().as_secs_f64());
            Ok(r)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

fn db() -> Result<&'static Db, String> {
    DB.get_or_init(|| {
        let t = std::time::Instant::now();
        let cards = crate::load_cards().map_err(|e| e.to_string())?;
        let index = Index::new(&cards);
        // `cards` está em ordem de popularidade. Percorrendo de trás para frente e
        // sobrescrevendo, quem fica no índice é a carta mais jogada. Primeiro os nomes de
        // face, depois os nomes completos, para um nome completo sempre ganhar de uma face
        // ("Fire" é face de Fire // Ice e de Start // Fire: fica Fire // Ice).
        let mut by_name = HashMap::new();
        for (i, c) in cards.iter().enumerate().rev() {
            if c.name.contains(" // ") {
                for face in c.name.split(" // ") {
                    by_name.insert(search::norm_name(face), i);
                }
            }
        }
        for (i, c) in cards.iter().enumerate().rev() {
            by_name.insert(search::norm_name(&c.name), i);
        }
        eprintln!("[mtg-rag] {} cartas carregadas em {:.1}s", cards.len(), t.elapsed().as_secs_f64());
        Ok(Db { cards, index, by_name })
    })
    .as_ref()
    .map_err(|e| e.clone())
}

const INSTRUCTIONS: &str = "MTG-RAG: busca local sobre todas as cartas de Magic: The Gathering (dados do Scryfall). \
Use estas ferramentas em vez de confiar na memória para texto, legalidade, preço ou existência de cartas. \
Dicas: (1) o texto de busca funciona melhor com palavras do texto oracle em inglês (ex.: 'sacrifice a creature draw'); \
(2) combine com filtros e tags — veja list_tags; tags podem ser qualificadas, ex. 'theme:tokens'; \
(3) sempre rode validate_deck antes de apresentar um deck ao usuário. \
Dúvidas de regra ou de situação de jogo: chame explain_interaction com as cartas envolvidas e a pergunta (traz texto, rulings oficiais, \
regras das keywords e regras relevantes das Comprehensive Rules); aprofunde com search_rules e get_rule. Rulings oficiais e regras \
têm prioridade sobre interpretação. Cite os números das regras (ex.: 613.1, 702.19b) para o usuário conferir no PDF.";

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
                    "commander": {"type": "string", "description": "Comandante (ou dois parceiros separados por ' + '). Fica fora da contagem da lista."},
                    "price_max": {"type": "number", "description": "Orçamento por carta em dólares: aponta as cartas acima dele (terrenos básicos e comandantes não contam) e as sem preço."},
                    "no_game_changers": {"type": "boolean", "description": "Trata Game Changers como problema (brackets 1-3 de Commander)."}
                },
                "required": ["decklist", "format"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "explain_interaction",
            "title": "Explicar interação/situação de jogo",
            "description": "Para dúvidas de regra numa situação de jogo. Junta, para cada carta: texto oracle, rulings oficiais (Wizards) e as Comprehensive Rules das keywords dela; e, se houver 'question', as regras mais relevantes para a pergunta. Também aponta os conceitos de regra que o texto das cartas já indica ('instead' = substituição 614.1a, \"can't\" = 101.2, mudança de tipo = camadas 613.1...). Use o resultado como fonte e cite os números das regras. A pergunta pode ser em português (termos de Magic são traduzidos) ou inglês.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cards": str_array("Cartas envolvidas (1 a 8 nomes)."),
                    "question": {"type": "string", "description": "A dúvida, em português ou inglês. Termos de regra ajudam (pilha, camadas, substituição, ações baseadas em estado)."}
                },
                "required": ["cards"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "search_rules",
            "title": "Buscar nas regras",
            "description": "Busca nas Comprehensive Rules (regras completas oficiais) e no glossário. Aceita português (termos de Magic são traduzidos) ou inglês ('trample assign damage', 'imposto de comandante', 'mulligan', 'camada 4 mudança de tipo'). Verbetes do glossário trazem junto a regra que citam. Um número de regra na consulta ('702.19b') vem primeiro.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 30, "description": "Padrão 8."}
                },
                "required": ["query"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "get_rule",
            "title": "Ler regra pelo número",
            "description": "Uma regra das Comprehensive Rules pelo número ('613.1', '702.19', '903'), com o contexto acima dela e todas as sub-regras e exemplos. Também aceita um termo do glossário ('Trample').",
            "inputSchema": {
                "type": "object",
                "properties": {"number": {"type": "string", "description": "Número da regra (ex.: '702.19b') ou termo do glossário."}},
                "required": ["number"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "get_rulings",
            "title": "Rulings da carta",
            "description": "Rulings oficiais de uma carta (esclarecimentos da Wizards e do Scryfall, com data), o texto oracle e as regras das keywords dela.",
            "inputSchema": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
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
// LLMs às vezes mandam números e booleanos como texto ("5", "true"): aceitamos os dois.
fn n(a: &Value, k: &str) -> Option<f64> {
    match a.get(k)? {
        Value::Number(x) => x.as_f64(),
        Value::String(x) => x.trim().replace(',', ".").parse().ok(),
        _ => None,
    }
}
fn u(a: &Value, k: &str) -> Option<u64> {
    n(a, k).filter(|x| *x >= 0.0).map(|x| x as u64)
}
fn b(a: &Value, k: &str) -> bool {
    match a.get(k) {
        Some(Value::Bool(x)) => *x,
        Some(Value::String(x)) => matches!(x.trim().to_lowercase().as_str(), "true" | "1" | "yes" | "sim"),
        Some(Value::Number(x)) => x.as_f64() == Some(1.0),
        _ => false,
    }
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
        top: u(a, "limit").unwrap_or(10).clamp(1, 50) as usize,
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
    let found = match db.exact(&name) {
        Some(c) => vec![c],
        None => search::find_by_name(&db.cards, &name),
    };
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
    // palavras genéricas de nome de carta não contam ("Gurmag the Destroyer" não deve
    // sugerir "Thomil, the Destroyer" na frente das cartas "Gurmag")
    const GENERIC: &[&str] = &["the", "of", "and", "from", "for", "with", "into", "a", "an"];
    let low = search::norm_name(name);
    let words: Vec<&str> = low
        .split(|ch: char| !ch.is_alphanumeric() && ch != '\'' && ch != '-')
        .filter(|w| w.len() >= 3 && !GENERIC.contains(w))
        .collect();
    let mut scored: Vec<(usize, u32, &str)> = db
        .cards
        .iter()
        .filter_map(|c| {
            let cn = search::norm_name(&c.name);
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
        per: u(a, "per_section").unwrap_or(8).clamp(1, 30) as usize,
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

/// Uma linha de lista de deck já interpretada.
#[derive(Debug, PartialEq)]
enum DeckLine {
    Skip,
    /// cabeçalho de seção: true = sideboard
    Section(bool),
    /// (quantidade, nome, esta linha é de sideboard por prefixo "SB:")
    Card(u32, String, bool),
}

/// Entende os formatos comuns: "4 Bolt", "4x Bolt", "Bolt", Arena "4 Bolt (M10) 146",
/// MTGO "SB: 2 Duress", Moxfield "1 Sol Ring (C21) 263 *F*" / "*CMDR*", cabeçalhos e comentários.
fn parse_deck_line(raw: &str) -> DeckLine {
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (card_re, marker_re) = RE.get_or_init(|| {
        (
            Regex::new(r"^(?:(\d+)\s*x?\s+)?(.+?)(?:\s+[\(\[][A-Za-z0-9]{2,6}[\)\]](?:\s+\S+)?)?\s*$").unwrap(),
            Regex::new(r"\s+\*[A-Za-z]+\*").unwrap(),
        )
    });
    let mut line = raw.trim();
    let low = line.to_lowercase();
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return if low.contains("sideboard") { DeckLine::Section(true) } else { DeckLine::Skip };
    }
    match low.trim_end_matches(':').trim() {
        "sideboard" | "side" => return DeckLine::Section(true),
        "deck" | "main" | "maindeck" | "main deck" | "commander" | "companion" | "about" => return DeckLine::Section(false),
        _ => {}
    }
    let mut side = false;
    if low.starts_with("sb:") {
        side = true;
        line = line[3..].trim();
    }
    let cleaned = marker_re.replace_all(line, "");
    let Some(cap) = card_re.captures(cleaned.trim()) else { return DeckLine::Skip };
    let qty = cap.get(1).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    DeckLine::Card(qty, cap[2].trim().to_string(), side)
}

/// Máximo de cópias da carta no formato. None = ilimitado.
fn copy_limit(c: &Card, format: &str, singleton: bool) -> Option<u32> {
    let text = c.oracle_text.to_lowercase();
    if c.supertypes.iter().any(|t| t == "Basic") || text.contains("a deck can have any number of cards named") {
        return None;
    }
    // "A deck can have up to seven cards named Seven Dwarves."
    if let Some(rest) = text.split("a deck can have up to ").nth(1) {
        let word = rest.split_whitespace().next().unwrap_or("");
        let n = match word {
            "two" => 2, "three" => 3, "four" => 4, "five" => 5, "six" => 6, "seven" => 7,
            "eight" => 8, "nine" => 9, "ten" => 10, w => w.parse().unwrap_or(4),
        };
        return Some(n);
    }
    if singleton || c.restricted_formats.iter().any(|f| f == format) {
        return Some(1);
    }
    Some(4)
}

fn validate_deck(db: &Db, a: &Value) -> Result<Value, String> {
    let text = s(a, "decklist").ok_or("informe 'decklist'")?;
    let format = s(a, "format").ok_or("informe 'format'")?.to_lowercase();
    let known: std::collections::BTreeSet<&str> =
        db.cards.iter().flat_map(|c| c.legal_formats.iter().map(String::as_str)).collect();
    if !known.contains(format.as_str()) {
        return Err(format!(
            "formato desconhecido: '{format}'. Formatos válidos: {}",
            known.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let singleton = SINGLETON.contains(&format.as_str());

    let find = |name: &str| -> Option<&Card> { db.exact(name) };

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
        let (qty, name, sb) = match parse_deck_line(raw) {
            DeckLine::Skip => continue,
            DeckLine::Section(s) => { in_side = s; continue; }
            DeckLine::Card(q, n, sb) => (q, n, sb),
        };
        match find(&name) {
            Some(c) => {
                let slot = if in_side || sb { &mut side } else { &mut main };
                slot.entry(c.name.clone()).or_insert((0, c)).0 += qty;
            }
            None => unknown.push(json!({"name": name, "suggestions": suggest_names(db, &name)})),
        }
    }

    // regras
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
        if let Some(max) = copy_limit(c, &format, singleton)
            && *q > max
        {
            let why = if c.restricted_formats.contains(&format) { " — restrita" } else { "" };
            problems.push(format!("{name}: {q} cópias (máximo {max} em {format}{why})"));
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
    if b(a, "no_game_changers") && !game_changers.is_empty() {
        problems.push(format!("Game Changers (pedido sem): {}", game_changers.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(limit) = n(a, "price_max") {
        let is_cmd = |c: &Card| commanders.iter().any(|x| x.oracle_id == c.oracle_id);
        let mut over: Vec<(String, f64)> = totals
            .values()
            .filter(|(_, c)| !c.supertypes.iter().any(|t| t == "Basic") && !is_cmd(c))
            .filter_map(|(_, c)| c.price_usd.filter(|p| *p > limit).map(|p| (c.name.clone(), p)))
            .collect();
        over.sort_by(|x, y| y.1.total_cmp(&x.1));
        if !over.is_empty() {
            problems.push(format!(
                "acima do orçamento de US${limit:.2} por carta: {}",
                over.iter().map(|(n, p)| format!("{n} (US${p:.2})")).collect::<Vec<_>>().join(", ")
            ));
        }
    }

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

// ---------------------------------------------------------------- regras

fn rule_json(db: &RulesDb, r: &rules::Rule) -> Value {
    let mut v = json!({"id": r.id, "context": db.breadcrumb(&r.id), "text": r.text});
    if !r.examples.is_empty() {
        v["examples"] = json!(r.examples);
    }
    v
}

fn rules_meta(db: &RulesDb) -> Value {
    json!({
        "comprehensive_rules_effective": db.doc.effective,
        // caminho absoluto: o usuário (ou o LLM) precisa achar o arquivo para conferir a regra
        "pdf": std::path::absolute(crate::data_dir().join(fetch::RULES_PDF))
            .map_or_else(|_| fetch::RULES_PDF.to_string(), |p| p.display().to_string()),
    })
}

fn search_rules(db: &RulesDb, a: &Value) -> Result<Value, String> {
    let q = s(a, "query").ok_or("informe 'query'")?;
    let limit = u(a, "limit").unwrap_or(8).clamp(1, 30) as usize;
    let hits: Vec<Value> = db
        .search(&q, limit)
        .into_iter()
        .map(|(_, h)| match h {
            RuleHit::Rule(r) => {
                let mut v = rule_json(db, r);
                v["type"] = json!("rule");
                v
            }
            RuleHit::Glossary(g) => json!({"type": "glossary", "term": g.term, "text": g.text}),
        })
        .collect();
    Ok(json!({"count": hits.len(), "results": hits, "meta": rules_meta(db)}))
}

/// Sub-regra sem o "context" (é o mesmo da regra pedida) para não repetir texto.
fn child_json(r: &rules::Rule) -> Value {
    let mut v = json!({"id": r.id, "text": r.text});
    if !r.examples.is_empty() {
        v["examples"] = json!(r.examples);
    }
    v
}

/// Seções grandes (ex.: 702 tem ~190 keywords) viram um índice: número + título/início.
const FULL_CHILDREN_MAX: usize = 30;

fn get_rule(db: &RulesDb, a: &Value) -> Result<Value, String> {
    let raw = s(a, "number").ok_or("informe 'number' (ex.: '702.19b')")?;
    let Some(id) = rules::find_rule_number(&raw) else {
        return match db.glossary(&raw) {
            Some(g) => Ok(json!({
                "glossary": {"term": g.term, "text": g.text},
                "see_rules": db.referenced(&g.text).iter().map(|r| {
                    let mut v = rule_json(db, r);
                    v["subrules"] = json!(db.children(&r.id).iter().take(FULL_CHILDREN_MAX).map(|c| child_json(c)).collect::<Vec<_>>());
                    v
                }).collect::<Vec<_>>(),
                "meta": rules_meta(db),
            })),
            None => Err(format!("'{raw}' não é número de regra nem termo do glossário; tente search_rules")),
        };
    };
    let r = db
        .get(&id)
        .ok_or_else(|| format!("a regra {id} não existe nas Comprehensive Rules de {}; tente search_rules", db.doc.effective))?;
    let children = db.children(&id);
    let total = children.len();
    let (subrules, note) = if total > FULL_CHILDREN_MAX {
        let idx: Vec<Value> = children
            .iter()
            .map(|c| {
                let short: String = c.text.chars().take(90).collect();
                json!({"id": c.id, "text": if c.text.chars().count() > 90 { format!("{short}…") } else { short }})
            })
            .collect();
        (idx, Some(format!("seção com {total} sub-regras: lista resumida; chame get_rule com o número de uma delas para o texto completo")))
    } else {
        (children.iter().map(|c| child_json(c)).collect(), None)
    };
    let mut out = json!({
        "rule": rule_json(db, r),
        "parents": db.ancestors(&id).iter().map(|p| json!({"id": p.id, "text": p.text})).collect::<Vec<_>>(),
        "subrule_count": total,
        "subrules": subrules,
        "meta": rules_meta(db),
    });
    if let Some(n) = note {
        out["note"] = json!(n);
    }
    Ok(out)
}

fn find_card<'a>(db: &'a Db, name: &str) -> Option<&'a Card> {
    db.exact(name).or_else(|| search::find_by_name(&db.cards, name).into_iter().next())
}

/// Carta + rulings + regras das keywords: o pacote de contexto para uma dúvida de regra.
fn card_rules_json(rdb: &RulesDb, c: &Card) -> Value {
    let keyword_rules: Vec<Value> = c
        .keywords
        .iter()
        .filter_map(|k| {
            let r = rdb.keyword_rule(k)?;
            let first: Vec<String> = rdb.children(&r.id).iter().take(3).map(|x| format!("{} {}", x.id, x.text)).collect();
            Some(json!({"keyword": k, "rule": r.id, "text": first}))
        })
        .collect();
    json!({
        "name": c.name, "mana_cost": c.mana_cost, "type_line": c.type_line, "oracle_text": c.oracle_text,
        "power": c.power, "toughness": c.toughness, "loyalty": c.loyalty, "keywords": c.keywords,
        "rulings": rdb.rulings_for(&c.oracle_id).iter().map(|r| json!({"date": r.date, "source": r.source, "text": r.text})).collect::<Vec<_>>(),
        "keyword_rules": keyword_rules,
    })
}

fn get_rulings(cards: &Db, rdb: &RulesDb, a: &Value) -> Result<Value, String> {
    let name = s(a, "name").ok_or("informe 'name'")?;
    match find_card(cards, &name) {
        Some(c) => Ok(json!({"card": card_rules_json(rdb, c), "meta": rules_meta(rdb)})),
        None => Err(format!("carta não encontrada: {name}. Parecidas: {}", suggest_names(cards, &name).join(", "))),
    }
}

/// Remove nomes de cartas de uma pergunta (sem diferenciar maiúsculas/acentos).
fn strip_card_names<'a>(question: &str, names: impl Iterator<Item = &'a str>) -> String {
    let mut q = search::norm_name(question);
    let mut ns: Vec<String> = names.map(search::norm_name).filter(|n| n.len() >= 3).collect();
    ns.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for n in ns {
        q = q.replace(&n, " ");
    }
    q
}

/// Conceitos de regra que o texto da carta já denuncia, com a regra que os define.
/// Ex.: "instead" = efeito de substituição (614.1a); "can't" ganha de "can" (101.2).
const CONCEPTS: &[(&str, &str, &str)] = &[
    (r"\binstead\b", "614.1a", "efeito de substituição ('instead')"),
    (r"\bcan't\b|\bcannot\b", "101.2", "'can't' tem prioridade sobre 'can'"),
    (r"\b(cast|play) [^.]*from (your|a|their) graveyard|\bcast [^.]*from exile", "601.3", "permissão para conjurar de outra zona"),
    (r"\bdies\b", "700.4", "o que significa 'dies'"),
    (r"\bcopy\b|\bcopies\b|\bas a copy\b", "707.2", "o que uma cópia copia"),
    (r"\bwhen(ever)? [^.]*\benters\b", "603.6a", "habilidade de entrar no campo"),
    (
        r"lose all abilities|\bbase power\b|in addition to (its|their) other types|\b(lands|creatures|permanents) are [a-z]+s\b|nonbasic lands are|\bis an? [a-z]+ (creature|land)\b",
        "613.1",
        "efeito contínuo que muda tipo/habilidades/P/R: sistema de camadas",
    ),
];

fn concept_rules(rdb: &RulesDb, cards: &[&Card]) -> Vec<Value> {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    let res = RES.get_or_init(|| CONCEPTS.iter().map(|(p, _, _)| Regex::new(p).unwrap()).collect());
    let mut out = vec![];
    for ((_, id, why), re) in CONCEPTS.iter().zip(res) {
        let hits: Vec<&str> = cards.iter().filter(|c| re.is_match(&c.oracle_text.to_lowercase())).map(|c| c.name.as_str()).collect();
        if hits.is_empty() {
            continue;
        }
        if let Some(r) = rdb.get(id) {
            let mut v = rule_json(rdb, r);
            v["why"] = json!(format!("{why} — em {}", hits.join(", ")));
            out.push(v);
        }
    }
    out
}

fn explain_interaction(cards: &Db, rdb: &RulesDb, a: &Value) -> Result<Value, String> {
    let names = list(a, "cards");
    if names.is_empty() || names.len() > 8 {
        return Err("informe de 1 a 8 cartas em 'cards'".into());
    }
    let mut found = vec![];
    let mut found_cards: Vec<&Card> = vec![];
    let mut unknown = vec![];
    for n in &names {
        match find_card(cards, n) {
            Some(c) => {
                found.push(card_rules_json(rdb, c));
                found_cards.push(c);
            }
            None => unknown.push(json!({"name": n, "suggestions": suggest_names(cards, n)})),
        }
    }
    // Os nomes das cartas atrapalham a busca nas regras ("Rest in Peace" casa com
    // "for the rest of the game", de Epic): tiramos antes de buscar.
    let related: Vec<Value> = s(a, "question")
        .map(|q| {
            let q = strip_card_names(&q, names.iter().map(String::as_str).chain(found_cards.iter().flat_map(|c| c.name.split(" // "))));
            rdb.search(&q, 8)
                .into_iter()
                .map(|(_, h)| match h {
                    RuleHit::Rule(r) => rule_json(rdb, r),
                    RuleHit::Glossary(g) => json!({"glossary": g.term, "text": g.text}),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "cards": found,
        "unknown": unknown,
        "concept_rules": concept_rules(rdb, &found_cards),
        "related_rules": related,
        "how_to_answer": "Responda com base nesses textos: rulings oficiais e regras têm prioridade sobre interpretação. Cite os números das regras. Se faltar uma regra (ex.: camadas 613, efeitos de substituição 614, pilha 405, ações baseadas em estado 704), busque com search_rules ou leia com get_rule.",
        "meta": rules_meta(rdb),
    }))
}

fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    if name == "list_tags" {
        return Ok(list_tags(db().ok()));
    }
    let rules_err = |e: String| format!("regras indisponíveis: {e}");
    let cards_err = |e: String| format!("base de cartas indisponível: {e}");
    match name {
        "search_rules" => return search_rules(rules_db().map_err(rules_err)?, args),
        "get_rule" => return get_rule(rules_db().map_err(rules_err)?, args),
        "get_rulings" => return get_rulings(db().map_err(cards_err)?, rules_db().map_err(rules_err)?, args),
        "explain_interaction" => return explain_interaction(db().map_err(cards_err)?, rules_db().map_err(rules_err)?, args),
        _ => {}
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
    std::thread::spawn(|| {
        if let Err(e) = rules_db() {
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
        assert_eq!(
            names,
            ["search_cards", "get_card", "suggest_deck", "validate_deck", "explain_interaction", "search_rules", "get_rule", "get_rulings", "list_tags"]
        );
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
    fn parses_common_decklist_formats() {
        use DeckLine::*;
        let card = |q, n: &str, sb| Card(q, n.to_string(), sb);
        assert_eq!(parse_deck_line("4 Lightning Bolt"), card(4, "Lightning Bolt", false));
        assert_eq!(parse_deck_line("4x Lightning Bolt"), card(4, "Lightning Bolt", false));
        assert_eq!(parse_deck_line("Sol Ring"), card(1, "Sol Ring", false));
        assert_eq!(parse_deck_line("4 Lightning Bolt (M10) 146"), card(4, "Lightning Bolt", false));
        assert_eq!(parse_deck_line("1 Sol Ring (C21) 263 *F*"), card(1, "Sol Ring", false));
        assert_eq!(parse_deck_line("1 Atraxa, Praetors' Voice *CMDR*"), card(1, "Atraxa, Praetors' Voice", false));
        assert_eq!(parse_deck_line("SB: 2 Duress"), card(2, "Duress", true));
        assert_eq!(parse_deck_line("1 Fire // Ice"), card(1, "Fire // Ice", false));
        assert_eq!(parse_deck_line("Sideboard"), Section(true));
        assert_eq!(parse_deck_line("SIDEBOARD:"), Section(true));
        assert_eq!(parse_deck_line("# SIDEBOARD"), Section(true));
        assert_eq!(parse_deck_line("Deck"), Section(false));
        assert_eq!(parse_deck_line("# Ramp"), Skip);
        assert_eq!(parse_deck_line("   "), Skip);
    }

    fn mk(name: &str, supertypes: &[&str], text: &str, restricted: &[&str]) -> crate::card::Card {
        serde_json::from_value(json!({
            "oracle_id": "", "name": name, "layout": "normal", "mana_cost": "", "cmc": 0.0, "type_line": "",
            "supertypes": supertypes, "types": [], "subtypes": [], "colors": [], "color_identity": [],
            "keywords": [], "power": null, "toughness": null, "loyalty": null, "defense": null,
            "produced_mana": [], "rarity": "common", "set": "", "set_name": "", "released_at": "",
            "legal_formats": ["vintage", "legacy"], "restricted_formats": restricted, "edhrec_rank": null,
            "game_changer": false, "reserved": false, "digital_only": false, "funny": false,
            "can_be_commander": false, "instant_speed": false, "price_usd": null, "price_eur": null,
            "oracle_text": text, "tags": {"function": [], "trigger": [], "target": [], "theme": []},
            "tribes": [], "scryfall_uri": null, "image": null, "search_text": ""
        }))
        .unwrap()
    }

    #[test]
    fn copy_limits() {
        let bolt = mk("Lightning Bolt", &[], "Lightning Bolt deals 3 damage to any target.", &[]);
        assert_eq!(copy_limit(&bolt, "legacy", false), Some(4));
        assert_eq!(copy_limit(&bolt, "commander", true), Some(1));
        let lotus = mk("Black Lotus", &[], "{T}, Sacrifice Black Lotus: Add three mana of any one color.", &["vintage"]);
        assert_eq!(copy_limit(&lotus, "vintage", false), Some(1));
        let swamp = mk("Snow-Covered Swamp", &["Basic", "Snow"], "({T}: Add {B}.)", &[]);
        assert_eq!(copy_limit(&swamp, "modern", false), None);
        let rats = mk("Relentless Rats", &[], "A deck can have any number of cards named Relentless Rats.", &[]);
        assert_eq!(copy_limit(&rats, "commander", true), None);
        let dwarves = mk("Seven Dwarves", &[], "Seven Dwarves gets +1/+1 for each other creature named Seven Dwarves you control.\nA deck can have up to seven cards named Seven Dwarves.", &[]);
        assert_eq!(copy_limit(&dwarves, "legacy", false), Some(7));
    }

    #[test]
    fn card_names_are_stripped_from_rule_questions() {
        let q = strip_card_names("Se Rest in Peace está em campo, posso conjurar Gravecrawler?", ["Rest in Peace", "Gravecrawler"].into_iter());
        assert!(!q.contains("rest") && !q.contains("peace") && !q.contains("gravecrawler"), "{q}");
        assert!(q.contains("conjurar") && q.contains("campo"));
    }

    #[test]
    fn concept_patterns_match_card_text() {
        let find = |text: &str| -> Vec<&str> {
            CONCEPTS.iter().filter(|(p, _, _)| Regex::new(p).unwrap().is_match(&text.to_lowercase())).map(|(_, id, _)| *id).collect()
        };
        assert_eq!(find("If a card or token would be put into a graveyard from anywhere, exile it instead."), ["614.1a"]);
        assert_eq!(find("You may cast this card from your graveyard as long as you control a Zombie."), ["601.3"]);
        assert_eq!(find("Nonbasic lands are Mountains."), ["613.1"]);
        assert_eq!(find("All creatures lose all abilities and have base power and toughness 1/1."), ["613.1"]);
        assert_eq!(find("Your opponents can't cast spells during your turn."), ["101.2"]);
        assert!(find("Flying").is_empty());
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
