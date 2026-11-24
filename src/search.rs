//! Passo 2 do RAG: busca híbrida (filtros exatos + tags + BM25 por palavras, com stemming).

use crate::card::Card;
use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::{HashMap, HashSet};

const STOP: &[&str] = &[
    "a", "an", "the", "of", "to", "and", "or", "in", "on", "for", "with", "your", "you", "is", "it", "its",
    "this", "that", "as", "at", "by", "be", "from", "are", "if", "de", "da", "do", "das", "dos", "e", "o",
    "os", "as", "um", "uma", "que", "com", "para", "por", "no", "na", "em", "se", "ao", "carta", "cartas",
];

/// Quebra o texto em palavras, tira stopwords e reduz ao radical
/// ("creatures", "creature" -> "creatur"; "dies", "died" -> "die").
pub struct Tokenizer {
    re: Regex,
    stop: HashSet<&'static str>,
    stem: Stemmer,
}

impl Tokenizer {
    pub fn new() -> Self {
        Tokenizer {
            re: Regex::new(r"[\p{L}\p{N}+\-/{}']+").unwrap(),
            stop: STOP.iter().copied().collect(),
            stem: Stemmer::create(Algorithm::English),
        }
    }

    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let low = text.to_lowercase();
        self.re
            .find_iter(&low)
            .map(|m| m.as_str().trim_end_matches("'s").trim_matches('\'').trim_matches('-'))
            .filter(|t| !t.is_empty() && !self.stop.contains(t))
            .map(|t| {
                if t.chars().all(char::is_alphabetic) { self.stem.stem(t).into_owned() } else { t.to_string() }
            })
            .collect()
    }
}

/// BM25 com índice invertido: só visita as cartas que têm as palavras da consulta.
pub struct Bm25 {
    vocab: HashMap<String, usize>,
    postings: Vec<Vec<(u32, f32)>>,
    idf: Vec<f64>,
    len: Vec<f32>,
    avg: f64,
}

const K1: f64 = 1.5;
const B: f64 = 0.75;

impl Bm25 {
    pub fn new(docs: impl Iterator<Item = Vec<String>>) -> Self {
        let mut vocab: HashMap<String, usize> = HashMap::new();
        let mut postings: Vec<Vec<(u32, f32)>> = vec![];
        let mut len = vec![];
        for (i, d) in docs.enumerate() {
            len.push(d.len() as f32);
            let mut counts: HashMap<usize, f32> = HashMap::new();
            for t in d {
                let next = vocab.len();
                let id = *vocab.entry(t).or_insert(next);
                if id == postings.len() {
                    postings.push(vec![]);
                }
                *counts.entry(id).or_default() += 1.0;
            }
            for (id, tf) in counts {
                postings[id].push((i as u32, tf));
            }
        }
        let n = len.len() as f64;
        let avg = len.iter().map(|&x| x as f64).sum::<f64>() / n.max(1.0);
        let idf = postings.iter().map(|p| (1.0 + (n - p.len() as f64 + 0.5) / (p.len() as f64 + 0.5)).ln()).collect();
        Bm25 { vocab, postings, idf, len, avg }
    }

    /// Nota de cada documento para a consulta (vetor do tamanho do corpus).
    pub fn scores(&self, query: &[String]) -> Vec<f64> {
        let mut out = vec![0.0; self.len.len()];
        for t in query {
            let Some(&id) = self.vocab.get(t) else { continue };
            let idf = self.idf[id];
            for &(doc, tf) in &self.postings[id] {
                let tf = tf as f64;
                let norm = 1.0 - B + B * self.len[doc as usize] as f64 / self.avg;
                out[doc as usize] += idf * tf * (K1 + 1.0) / (tf + K1 * norm);
            }
        }
        out
    }
}

/// Índice de busca montado uma vez por execução.
pub struct Index {
    pub tok: Tokenizer,
    pub bm25: Bm25,
}

impl Index {
    pub fn new(cards: &[Card]) -> Self {
        let tok = Tokenizer::new();
        let bm25 = Bm25::new(cards.iter().map(|c| tok.tokenize(&c.search_text)));
        Index { tok, bm25 }
    }
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum Sort {
    #[default]
    Relevance,
    Popularity,
    Cmc,
    Price,
    Name,
    Newest,
}

impl Sort {
    pub fn parse(s: &str) -> Result<Sort, String> {
        Ok(match s {
            "relevance" | "relevancia" => Sort::Relevance,
            "popularity" | "popularidade" | "edhrec" => Sort::Popularity,
            "cmc" => Sort::Cmc,
            "price" | "preco" => Sort::Price,
            "name" | "nome" => Sort::Name,
            "newest" | "recentes" => Sort::Newest,
            _ => return Err(format!("--sort inválido: {s} (relevance|popularity|cmc|price|name|newest)")),
        })
    }
}

#[derive(Default)]
pub struct Query {
    pub text: String,
    /// identidade de cor permitida (subconjunto); "C" = só incolores
    pub colors: Option<Vec<String>>,
    /// cores que a identidade precisa conter
    pub colors_include: Vec<String>,
    pub cmc_min: Option<f64>,
    pub cmc_max: Option<f64>,
    pub types: Vec<String>,
    pub subtype: Option<String>,
    pub tribe: Option<String>,
    pub rarity: Option<String>,
    pub name: Option<String>,
    /// exige todas
    pub tags: Vec<String>,
    /// exige ao menos uma
    pub any_tags: Vec<String>,
    /// exclui
    pub not_tags: Vec<String>,
    /// só aumenta a nota
    pub boost: Vec<String>,
    pub format: Option<String>,
    pub commander_only: bool,
    pub instant_speed: bool,
    pub price_max: Option<f64>,
    pub paper: bool,
    pub no_game_changers: bool,
    pub top: usize,
    pub sort: Sort,
}

impl Query {
    pub fn has_text(&self) -> bool {
        !self.text.trim().is_empty() || !self.boost.is_empty()
    }
}

/// Filtros exatos (o "WHERE" do SQL).
pub fn passes(c: &Card, q: &Query) -> bool {
    let ci = &c.color_identity;
    q.colors.as_ref().is_none_or(|cols| ci.iter().all(|x| cols.contains(x)))
        && q.colors_include.iter().all(|x| ci.contains(x))
        && q.cmc_min.is_none_or(|m| c.cmc >= m)
        && q.cmc_max.is_none_or(|m| c.cmc <= m)
        // vários --type = "ou" (Instant ou Sorcery)
        && (q.types.is_empty() || q.types.iter().any(|t| c.has_type(t)))
        && q.subtype.as_ref().is_none_or(|s| c.subtypes.iter().any(|x| x.eq_ignore_ascii_case(s)))
        && q.tribe.as_ref().is_none_or(|s| {
            c.tribes.iter().chain(&c.subtypes).any(|x| x.eq_ignore_ascii_case(s))
        })
        && q.rarity.as_ref().is_none_or(|r| c.rarity.eq_ignore_ascii_case(r))
        && q.name.as_ref().is_none_or(|n| c.name.to_lowercase().contains(&n.to_lowercase()))
        && q.format.as_ref().is_none_or(|f| c.is_legal(f))
        && (!q.commander_only || c.can_be_commander)
        && (!q.instant_speed || c.instant_speed)
        && q.price_max.is_none_or(|p| c.price_usd.is_some_and(|x| x <= p))
        && (!q.paper || (!c.digital_only && !c.funny))
        && (!q.no_game_changers || !c.game_changer)
        && q.tags.iter().all(|t| c.tags.contains(t))
        && (q.any_tags.is_empty() || q.any_tags.iter().any(|t| c.tags.contains(t)))
        && !q.not_tags.iter().any(|t| c.tags.contains(t))
}

pub struct Hit<'a> {
    pub score: f64,
    pub card: &'a Card,
}

fn rank(c: &Card) -> u32 {
    c.edhrec_rank.unwrap_or(u32::MAX)
}

pub fn search<'a>(cards: &'a [Card], index: &Index, q: &Query) -> Vec<Hit<'a>> {
    let has_text = q.has_text();
    let scores = if has_text { index.bm25.scores(&index.tok.tokenize(&q.text)) } else { vec![] };
    let wanted_name = q.text.trim().to_lowercase();

    let mut hits: Vec<Hit> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| passes(c, q))
        .filter_map(|(i, c)| {
            if !has_text {
                return Some(Hit { score: 0.0, card: c });
            }
            let mut s = scores[i] + 3.0 * q.boost.iter().filter(|t| c.tags.contains(t)).count() as f64;
            if c.name.to_lowercase() == wanted_name {
                s += 50.0; // o usuário digitou o nome exato
            }
            (s > 0.0).then_some(Hit { score: s, card: c })
        })
        .collect();

    let sort = if !has_text && q.sort == Sort::Relevance { Sort::Popularity } else { q.sort };
    match sort {
        Sort::Relevance => hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(rank(a.card).cmp(&rank(b.card)))),
        Sort::Popularity => hits.sort_by_key(|h| rank(h.card)),
        Sort::Cmc => hits.sort_by(|a, b| a.card.cmc.total_cmp(&b.card.cmc).then(rank(a.card).cmp(&rank(b.card)))),
        Sort::Price => hits.sort_by(|a, b| {
            a.card.price_usd.unwrap_or(f64::MAX).total_cmp(&b.card.price_usd.unwrap_or(f64::MAX))
        }),
        Sort::Name => hits.sort_by(|a, b| a.card.name.cmp(&b.card.name)),
        Sort::Newest => hits.sort_by(|a, b| b.card.released_at.cmp(&a.card.released_at)),
    }
    hits.truncate(q.top);
    hits
}

/// Acha uma carta pelo nome: exato (sem diferenciar maiúsculas), depois prefixo, depois "contém".
pub fn find_by_name<'a>(cards: &'a [Card], name: &str) -> Vec<&'a Card> {
    let n = name.trim().to_lowercase();
    let exact: Vec<&Card> = cards
        .iter()
        .filter(|c| c.name.to_lowercase() == n || c.name.to_lowercase().split(" // ").any(|f| f == n))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let mut part: Vec<&Card> = cards.iter().filter(|c| c.name.to_lowercase().contains(&n)).collect();
    part.sort_by_key(|c| (!c.name.to_lowercase().starts_with(&n), rank(c)));
    part
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stemming_matches_plurals_and_tenses() {
        let t = Tokenizer::new();
        assert_eq!(t.tokenize("creatures dies"), t.tokenize("creature died"));
        assert_eq!(t.tokenize("draws cards"), t.tokenize("draw card"));
        assert!(t.tokenize("{T}: Add {G}").contains(&"{t}".to_string()));
    }

    #[test]
    fn bm25_prefers_rare_words() {
        let t = Tokenizer::new();
        let docs = ["destroy target creature", "draw a card", "destroy target artifact"];
        let b = Bm25::new(docs.iter().map(|d| t.tokenize(d)));
        let s = b.scores(&t.tokenize("destroy artifact"));
        assert!(s[2] > s[0] && s[0] > s[1]);
    }
}
