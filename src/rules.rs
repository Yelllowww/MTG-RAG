//! Regras do jogo: Comprehensive Rules (Wizards) + rulings por carta (Scryfall).
//!
//! * `parse` lê o TXT oficial e separa capítulos ("7"), seções ("702"), regras
//!   ("702.19") e sub-regras ("702.19b"), com os exemplos e o glossário.
//! * `build` gera data/rules.json e data/rulings.json (rulings agrupados pelo
//!   oracle_id, o mesmo campo das cartas).
//! * `RulesDb` carrega os dois, monta a busca (BM25) e responde por número,
//!   por texto, por keyword de carta e por carta.

use crate::search::{Bm25, Tokenizer};
use flate2::read::MultiGzDecoder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read};
use std::path::Path;

pub const RULES_JSON: &str = "rules.json";
pub const RULINGS_JSON: &str = "rulings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// "7", "702", "702.19", "702.19b"
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub term: String,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RulesDoc {
    /// "November 6, 2026"
    pub effective: String,
    pub rules: Vec<Rule>,
    pub glossary: Vec<GlossaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ruling {
    pub source: String,
    pub date: String,
    pub text: String,
}

// ---------------------------------------------------------------- parser

/// Lê o TXT das Comprehensive Rules.
pub fn parse(raw: &str) -> RulesDoc {
    let text = raw.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().collect();
    let effective = lines
        .iter()
        .find_map(|l| l.trim().strip_prefix("These rules are effective as of "))
        .map(|s| s.trim_end_matches('.').to_string())
        .unwrap_or_default();

    // O sumário (Contents) também lista "1. Game Concepts"..."Glossary"/"Credits".
    // O corpo começa na SEGUNDA ocorrência de "1. Game Concepts"; o glossário,
    // na segunda de "Glossary"; e termina na segunda de "Credits".
    let nth = |needle: &str, n: usize| lines.iter().enumerate().filter(|(_, l)| l.trim() == needle).nth(n).map(|(i, _)| i);
    let body_start = nth("1. Game Concepts", 1).or(nth("1. Game Concepts", 0)).unwrap_or(0);
    let gloss_start = nth("Glossary", 1).or(nth("Glossary", 0)).unwrap_or(lines.len());
    let gloss_end = nth("Credits", 1).unwrap_or(lines.len());

    let chapter = Regex::new(r"^(\d)\. (.+)$").unwrap();
    let section = Regex::new(r"^(\d{3})\. (.+)$").unwrap();
    // o ponto final às vezes falta no próprio documento oficial ("606.5 If the total cost...")
    let rule = Regex::new(r"^(\d{3}\.\d+)\.? (.+)$").unwrap();
    let subrule = Regex::new(r"^(\d{3}\.\d+[a-z]+)\.? (.+)$").unwrap();

    let mut rules: Vec<Rule> = vec![];
    for line in &lines[body_start..gloss_start] {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let cap = subrule.captures(l).or_else(|| rule.captures(l)).or_else(|| section.captures(l)).or_else(|| chapter.captures(l));
        if let Some(c) = cap {
            rules.push(Rule { id: c[1].to_string(), text: c[2].trim().to_string(), examples: vec![] });
        } else if let Some(last) = rules.last_mut() {
            if let Some(ex) = l.strip_prefix("Example:") {
                last.examples.push(ex.trim().to_string());
            } else if let Some(prev) = last.examples.last_mut() {
                prev.push('\n');
                prev.push_str(l);
            } else {
                last.text.push('\n');
                last.text.push_str(l);
            }
        }
    }

    let mut glossary = vec![];
    for block in lines[(gloss_start + 1).min(lines.len())..gloss_end].split(|l| l.trim().is_empty()) {
        let mut it = block.iter().map(|l| l.trim());
        if let Some(term) = it.next() {
            let def: Vec<&str> = it.collect();
            if !def.is_empty() {
                glossary.push(GlossaryEntry { term: term.to_string(), text: def.join("\n") });
            }
        }
    }
    RulesDoc { effective, rules, glossary }
}

/// "702.19b" -> "702.19" -> "702" -> "7" -> None
pub fn parent_id(id: &str) -> Option<String> {
    if let Some(i) = id.find(|c: char| c.is_ascii_lowercase()) {
        return Some(id[..i].to_string());
    }
    if let Some((sec, _)) = id.split_once('.') {
        return Some(sec.to_string());
    }
    if id.len() == 3 {
        return Some(id[..1].to_string());
    }
    None
}

/// Primeiro número de regra num texto: "regra 702.19b", "CR 613.1", "702.19".
/// Vocabulário de regras em português -> inglês (chaves sem acento). As regras só
/// existem em inglês; sem isto, "imposto de comandante" não acha nada.
const PT_EN: &[(&str, &str)] = &[
    ("cemiterio", "graveyard"), ("conjurar", "cast"), ("conjuro", "cast"), ("conjura", "cast"), ("conjurada", "cast"),
    ("conjurado", "cast"), ("pilha", "stack"), ("camada", "layer"), ("camadas", "layers"), ("dano", "damage"),
    ("comandante", "commander"), ("terreno", "land"), ("terrenos", "lands"), ("criatura", "creature"),
    ("criaturas", "creatures"), ("magica", "spell"), ("magicas", "spells"), ("habilidade", "ability"),
    ("habilidades", "abilities"), ("desvirar", "untap"), ("desvira", "untap"), ("virar", "tap"), ("virada", "tapped"),
    ("virado", "tapped"), ("atacar", "attack"), ("ataca", "attacks"), ("ataque", "attack"), ("atacante", "attacking"),
    ("bloquear", "block"), ("bloqueia", "blocks"), ("bloqueio", "block"), ("bloqueadora", "blocking"),
    ("bloqueador", "blocking"), ("bloqueada", "blocked"), ("bloqueado", "blocked"), ("comprar", "draw"),
    ("compra", "draw"), ("grimorio", "library"), ("biblioteca", "library"), ("mao", "hand"), ("exilio", "exile"),
    ("exilar", "exile"), ("exilada", "exiled"), ("exilado", "exiled"), ("sacrificar", "sacrifice"),
    ("sacrifico", "sacrifice"), ("sacrificio", "sacrifice"), ("destruir", "destroy"), ("destruida", "destroyed"),
    ("destruido", "destroyed"), ("anular", "counter"), ("anulada", "countered"), ("alvo", "target"),
    ("alvos", "targets"), ("marcador", "counter"), ("marcadores", "counters"), ("ficha", "token"), ("fichas", "tokens"),
    ("turno", "turn"), ("fase", "phase"), ("etapa", "step"), ("manutencao", "upkeep"), ("combate", "combat"),
    ("vida", "life"), ("veneno", "poison"), ("copiar", "copy"), ("copia", "copy"), ("encantamento", "enchantment"),
    ("artefato", "artifact"), ("substituicao", "replacement"), ("substituir", "replace"), ("gatilho", "trigger"),
    ("dispara", "triggers"), ("disparar", "trigger"), ("resolver", "resolve"), ("resolve", "resolves"),
    ("custo", "cost"), ("imposto", "tax"), ("zona", "zone"), ("comando", "command"), ("jogador", "player"),
    ("oponente", "opponent"), ("perder", "lose"), ("perde", "loses"), ("ganhar", "gain"), ("vencer", "win"),
    ("jogo", "game"), ("partida", "game"), ("descartar", "discard"), ("descarte", "discard"),
    ("embaralhar", "shuffle"), ("topo", "top"), ("fundo", "bottom"), ("permanente", "permanent"),
    ("feitico", "sorcery"), ("instantaneo", "instant"), ("lendaria", "legendary"), ("lendario", "legendary"),
    ("prioridade", "priority"), ("resposta", "response"), ("responder", "respond"), ("baralho", "deck"),
    ("prevenir", "prevent"), ("morre", "dies"), ("morrer", "dies"), ("campo", "battlefield"), ("entra", "enters"),
    ("entrar", "enter"), ("sai", "leaves"), ("sair", "leave"), ("controle", "control"), ("controlar", "control"),
    ("dono", "owner"), ("acoes", "actions"), ("estado", "state"), ("efeito", "effect"), ("efeitos", "effects"),
    ("continuo", "continuous"), ("dependencia", "dependency"), ("protecao", "protection"), ("voar", "flying"),
    ("voo", "flying"), ("atropelar", "trample"), ("mortal", "deathtouch"), ("monarca", "monarch"),
    ("regenerar", "regenerate"), ("transformar", "transform"), ("cartas", "cards"), ("carta", "card"),
    ("poder", "power"), ("resistencia", "toughness"), ("letal", "lethal"), ("ativar", "activate"),
    ("ativada", "activated"), ("desencadeada", "triggered"), ("estatica", "static"), ("multijogador", "multiplayer"),
];

/// Acrescenta a tradução dos termos em português (mantém o texto original).
pub fn expand_pt(query: &str) -> String {
    let mut extra = vec![];
    for w in query.split(|c: char| !c.is_alphanumeric() && !"áàâãéêíóôõúüç".contains(c)) {
        let key = crate::search::norm_name(w);
        if let Some((_, en)) = PT_EN.iter().find(|(pt, _)| *pt == key) {
            extra.push(*en);
        }
    }
    if extra.is_empty() { query.to_string() } else { format!("{query} {}", extra.join(" ")) }
}

pub fn find_rule_number(s: &str) -> Option<String> {
    let re = Regex::new(r"\b(\d{3}(?:\.\d+[a-z]*)?|\d)\b").unwrap();
    re.captures(s.trim()).map(|c| c[1].trim_end_matches('.').to_string())
}

// ---------------------------------------------------------------- build

/// TXT das regras -> rules.json; bulk de rulings -> rulings.json (agrupado por oracle_id).
pub fn build(dir: &Path) -> Result<(), Box<dyn Error>> {
    let txt_path = dir.join(crate::fetch::RULES_TXT);
    match fs::read(&txt_path) {
        Ok(bytes) => {
            let doc = parse(&String::from_utf8_lossy(&bytes));
            if doc.rules.len() < 1000 {
                return Err(format!("o TXT das regras parece incompleto: só {} regras lidas", doc.rules.len()).into());
            }
            serde_json::to_writer(BufWriter::new(File::create(dir.join(RULES_JSON))?), &doc)?;
            println!("{} regras e {} verbetes de glossário (em vigor desde {})", doc.rules.len(), doc.glossary.len(), doc.effective);
        }
        Err(_) => println!("aviso: {} não existe; rode `magic_rag install` para baixar as regras", txt_path.display()),
    }

    let raw_path = dir.join(crate::fetch::RULINGS_RAW);
    if let Ok(f) = File::open(&raw_path) {
        #[derive(Deserialize)]
        struct Raw {
            oracle_id: String,
            source: String,
            published_at: String,
            comment: String,
        }
        let mut bytes = vec![];
        let mut f = f;
        f.read_to_end(&mut bytes)?;
        let text = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut s = String::new();
            MultiGzDecoder::new(&bytes[..]).read_to_string(&mut s)?;
            s
        } else {
            String::from_utf8(bytes)?
        };
        let raws: Vec<Raw> = if text.trim_start().starts_with('[') {
            serde_json::from_str(&text)?
        } else {
            BufReader::new(text.as_bytes()).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(&l)).collect::<Result<_, _>>()?
        };
        let total = raws.len();
        let mut by_card: BTreeMap<String, Vec<Ruling>> = BTreeMap::new();
        for r in raws {
            by_card.entry(r.oracle_id).or_default().push(Ruling { source: r.source, date: r.published_at, text: r.comment });
        }
        for v in by_card.values_mut() {
            v.sort_by(|a, b| a.date.cmp(&b.date));
        }
        serde_json::to_writer(BufWriter::new(File::create(dir.join(RULINGS_JSON))?), &by_card)?;
        println!("{total} rulings de {} cartas", by_card.len());
    } else {
        println!("aviso: {} não existe; rode `magic_rag install` para baixar os rulings", raw_path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------- consulta

/// Resultado de busca nas regras.
pub enum RuleHit<'a> {
    Rule(&'a Rule),
    Glossary(&'a GlossaryEntry),
}

pub struct RulesDb {
    pub doc: RulesDoc,
    pub rulings: HashMap<String, Vec<Ruling>>,
    by_id: HashMap<String, usize>,
    /// keyword em minúsculas ("trample", "dredge") -> id da regra ("702.19")
    keywords: HashMap<String, String>,
    tok: Tokenizer,
    bm25: Bm25,
}

impl RulesDb {
    pub fn load(dir: &Path) -> Result<RulesDb, String> {
        let rules_path = dir.join(RULES_JSON);
        let doc: RulesDoc = serde_json::from_reader(BufReader::new(
            File::open(&rules_path).map_err(|e| format!("{}: {e} (rode `magic_rag install`)", rules_path.display()))?,
        ))
        .map_err(|e| e.to_string())?;
        let rulings = File::open(dir.join(RULINGS_JSON))
            .ok()
            .and_then(|f| serde_json::from_reader(BufReader::new(f)).ok())
            .unwrap_or_default();
        Ok(Self::new(doc, rulings))
    }

    pub fn new(doc: RulesDoc, rulings: HashMap<String, Vec<Ruling>>) -> RulesDb {
        let by_id = doc.rules.iter().enumerate().map(|(i, r)| (r.id.clone(), i)).collect();
        // 701.x (keyword actions) e 702.x (keyword abilities): o texto da regra é o nome
        let kw_re = Regex::new(r"^70[12]\.\d+$").unwrap();
        let keywords = doc
            .rules
            .iter()
            .filter(|r| kw_re.is_match(&r.id))
            .map(|r| (r.text.to_lowercase(), r.id.clone()))
            .collect();
        let tok = Tokenizer::new();
        let mut db = RulesDb { doc, rulings, by_id, keywords, tok, bm25: Bm25::new(std::iter::empty()) };
        let docs: Vec<Vec<String>> = db
            .doc
            .rules
            .iter()
            .map(|r| db.tok.tokenize(&format!("{} {} {} {}", r.id, db.breadcrumb(&r.id), r.text, r.examples.join(" "))))
            .chain(db.doc.glossary.iter().map(|g| db.tok.tokenize(&format!("{} {} {}", g.term, g.term, g.text))))
            .collect();
        db.bm25 = Bm25::new(docs.into_iter());
        db
    }

    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.by_id.get(id).map(|&i| &self.doc.rules[i])
    }

    /// Regras-pai, da mais geral para a mais específica (sem a própria).
    pub fn ancestors(&self, id: &str) -> Vec<&Rule> {
        let mut out = vec![];
        let mut cur = parent_id(id);
        while let Some(p) = cur {
            if let Some(r) = self.get(&p) {
                out.push(r);
            }
            cur = parent_id(&p);
        }
        out.reverse();
        out
    }

    /// Filhas diretas: de "702" vêm "702.1".."702.190"; de "702.19" vêm "702.19a"...
    pub fn children(&self, id: &str) -> Vec<&Rule> {
        self.doc.rules.iter().filter(|r| parent_id(&r.id).as_deref() == Some(id)).collect()
    }

    /// "702 Keyword Abilities › 702.19 Trample" (títulos curtos dos pais).
    pub fn breadcrumb(&self, id: &str) -> String {
        let mut parts: Vec<String> = self
            .ancestors(id)
            .into_iter()
            .filter(|r| r.text.len() <= 60)
            .map(|r| format!("{} {}", r.id, r.text))
            .collect();
        if let Some(r) = self.get(id).filter(|r| r.text.len() <= 60 && !r.text.contains('.')) {
            parts.push(format!("{} {}", r.id, r.text));
        }
        parts.join(" › ")
    }

    /// Regra de uma keyword de carta ("Trample" -> 702.19).
    pub fn keyword_rule(&self, keyword: &str) -> Option<&Rule> {
        self.keywords.get(&keyword.to_lowercase()).and_then(|id| self.get(id))
    }

    pub fn glossary(&self, term: &str) -> Option<&GlossaryEntry> {
        let t = term.trim().to_lowercase();
        self.doc.glossary.iter().find(|g| g.term.to_lowercase() == t)
    }

    /// Regras citadas por um texto ("See rule 702.11" -> 702.11), que existam.
    pub fn referenced(&self, text: &str) -> Vec<&Rule> {
        static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"\brules? (\d{3}(?:\.\d+[a-z]?)?)").unwrap());
        let mut seen = std::collections::HashSet::new();
        re.captures_iter(text).filter_map(|c| self.get(&c[1])).filter(|r| seen.insert(r.id.clone())).collect()
    }

    /// Busca por texto nas regras e no glossário.
    /// * Um número de regra na consulta ("702.19b") vem sempre primeiro.
    /// * Termos de Magic em português são traduzidos (as regras só existem em inglês).
    /// * Um verbete de glossário traz junto a regra que ele cita ("See rule 103.5").
    pub fn search(&self, query: &str, limit: usize) -> Vec<(f64, RuleHit<'_>)> {
        let mut out: Vec<(f64, RuleHit)> = vec![];
        let mut seen = std::collections::HashSet::new();
        if let Some(n) = find_rule_number(query).filter(|n| n.contains('.') || n.len() == 3)
            && let Some(r) = self.get(&n)
        {
            seen.insert(r.id.clone());
            out.push((f64::MAX, RuleHit::Rule(r)));
        }
        let scores = self.bm25.scores(&self.tok.tokenize(&expand_pt(query)));
        let n_rules = self.doc.rules.len();
        let mut ranked: Vec<(usize, f64)> = scores.iter().copied().enumerate().filter(|(_, s)| *s > 0.0).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (i, s) in ranked {
            if out.len() >= limit {
                break;
            }
            if i < n_rules {
                let r = &self.doc.rules[i];
                if seen.insert(r.id.clone()) {
                    out.push((s, RuleHit::Rule(r)));
                }
            } else {
                let g = &self.doc.glossary[i - n_rules];
                out.push((s, RuleHit::Glossary(g)));
                for r in self.referenced(&g.text) {
                    if seen.insert(r.id.clone()) {
                        out.push((s, RuleHit::Rule(r)));
                    }
                }
            }
        }
        out.truncate(limit);
        out
    }

    pub fn rulings_for(&self, oracle_id: &str) -> &[Ruling] {
        self.rulings.get(oracle_id).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}Magic: The Gathering Comprehensive Rules

These rules are effective as of November 6, 2026.

Contents

1. Game Concepts
7. Additional Rules
Glossary
Credits

1. Game Concepts

100. General

100.1. These Magic rules apply to any Magic game with two or more players.

100.1a A two-player game is a game that begins with only two players.

7. Additional Rules

702. Keyword Abilities

702.19. Trample

702.19a Trample is a static ability that modifies the rules for assigning an attacking creature’s combat damage.

702.19b The controller of an attacking creature with trample first assigns damage to the creature(s) blocking it.
Example: A 6/6 green creature with trample is blocked by a 2/2 creature.
It assigns 2 damage to the blocker and 4 to the player.

702.52 Dredge

702.52a Dredge is a static ability that functions only while the card with dredge is in a player’s graveyard.

Glossary

Trample
A keyword ability that modifies how a creature assigns combat damage. See rule 702.19, “Trample.”

Ability
1. Text on an object that explains what that object does or can do.
2. An activated or triggered ability on the stack.

Credits

Magic: The Gathering Original Game Design: Richard Garfield
";

    #[test]
    fn parses_hierarchy_examples_and_glossary() {
        let doc = parse(SAMPLE);
        assert_eq!(doc.effective, "November 6, 2026");
        let ids: Vec<&str> = doc.rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["1", "100", "100.1", "100.1a", "7", "702", "702.19", "702.19a", "702.19b", "702.52", "702.52a"]);
        let b = doc.rules.iter().find(|r| r.id == "702.19b").unwrap();
        assert_eq!(b.examples.len(), 1);
        assert!(b.examples[0].contains("4 to the player"), "exemplo de várias linhas: {:?}", b.examples);
        assert_eq!(doc.glossary.len(), 2);
        assert_eq!(doc.glossary[1].term, "Ability");
        assert!(doc.glossary[1].text.contains("2. An activated"));
    }

    #[test]
    fn hierarchy_helpers() {
        assert_eq!(parent_id("702.19b").as_deref(), Some("702.19"));
        assert_eq!(parent_id("702.19").as_deref(), Some("702"));
        assert_eq!(parent_id("702").as_deref(), Some("7"));
        assert_eq!(parent_id("7"), None);
        assert_eq!(find_rule_number("regra 702.19b, por favor").as_deref(), Some("702.19b"));
        assert_eq!(find_rule_number("CR 613.1.").as_deref(), Some("613.1"));
    }

    #[test]
    fn lookup_search_and_keywords() {
        let db = RulesDb::new(parse(SAMPLE), HashMap::new());
        assert_eq!(db.keyword_rule("Trample").unwrap().id, "702.19");
        assert_eq!(db.keyword_rule("dredge").unwrap().id, "702.52");
        assert_eq!(db.breadcrumb("702.19b"), "7 Additional Rules › 702 Keyword Abilities › 702.19 Trample");
        let kids: Vec<&str> = db.children("702.19").iter().map(|r| r.id.as_str()).collect();
        assert_eq!(kids, ["702.19a", "702.19b"]);
        let hits = db.search("trample assign damage blocking creature", 3);
        assert!(matches!(&hits[0].1, RuleHit::Rule(r) if r.id.starts_with("702.19")));
        let hits = db.search("o que diz a 100.1a?", 3);
        assert!(matches!(&hits[0].1, RuleHit::Rule(r) if r.id == "100.1a"), "número na consulta vem primeiro");
        assert!(db.glossary("trample").is_some());
    }

    #[test]
    fn portuguese_terms_are_translated() {
        let q = expand_pt("imposto de comandante na zona de comando");
        assert!(q.contains("tax") && q.contains("commander") && q.contains("zone") && q.contains("command"), "{q}");
        let q = expand_pt("Criatura com atropelar bloqueada, dano letal?");
        assert!(q.contains("creature") && q.contains("trample") && q.contains("blocked") && q.contains("lethal"), "{q}");
        assert_eq!(expand_pt("trample damage"), "trample damage", "inglês fica como está");
        let db = RulesDb::new(parse(SAMPLE), HashMap::new());
        let hits = db.search("atropelar dano bloqueada", 3);
        assert!(matches!(&hits[0].1, RuleHit::Rule(r) if r.id.starts_with("702.19")), "busca em português");
    }

    #[test]
    fn glossary_hit_brings_the_rule_it_cites() {
        let db = RulesDb::new(parse(SAMPLE), HashMap::new());
        let refs: Vec<&str> = db.referenced("See rule 702.19, “Trample.” See also rule 702.19.").iter().map(|r| r.id.as_str()).collect();
        assert_eq!(refs, ["702.19"], "sem repetir");
        // "modifies how a creature assigns" só aparece no glossário; a regra 702.19 vem pela citação
        let hits = db.search("keyword ability modifies how assigns", 10);
        let pos_g = hits.iter().position(|(_, h)| matches!(h, RuleHit::Glossary(g) if g.term == "Trample")).unwrap();
        assert!(hits.iter().any(|(_, h)| matches!(h, RuleHit::Rule(r) if r.id == "702.19")), "a regra citada vem junto");
        let ids: Vec<String> = hits.iter().filter_map(|(_, h)| match h { RuleHit::Rule(r) => Some(r.id.clone()), _ => None }).collect();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids.len(), dedup.len(), "sem regra repetida");
        assert!(pos_g < hits.len());
    }
}
