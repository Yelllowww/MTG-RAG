//! Ajuda para montar deck de Commander: dado um comandante, sugere cartas por
//! categoria (ramp, compra, remoção...) e pelos temas do próprio comandante.
//!
//! Nota = sinergia com o texto do comandante (BM25) + popularidade no EDHREC.
//! A ideia é entregar ao LLM uma lista curta e boa; quem escolhe é ele/você.

use crate::card::Card;
use crate::search::{self, Index};
use std::collections::HashSet;

pub struct DeckOpts {
    pub per: usize,
    pub budget: Option<f64>,
    pub no_game_changers: bool,
    pub extra_tags: Vec<String>,
}

pub struct Section<'a> {
    pub key: String,
    pub title: String,
    pub cards: Vec<(f64, &'a Card)>,
}

pub struct DeckPlan<'a> {
    pub commander: &'a Card,
    pub sections: Vec<Section<'a>>,
}

const BASE: &[(&str, &str)] = &[
    ("ramp", "Ramp (aceleração de mana)"),
    ("card_draw", "Compra de cartas"),
    ("removal", "Remoção pontual"),
    ("board_wipe", "Board wipes"),
    ("protection", "Proteção"),
    ("tutor", "Tutores"),
];

fn popularity(c: &Card) -> f64 {
    match c.edhrec_rank {
        Some(r) => (1.0 - (r as f64).ln() / 40_000f64.ln()).clamp(0.0, 1.0),
        None => 0.0,
    }
}

pub fn build<'a>(cards: &'a [Card], index: &Index, commander_name: &str, o: &DeckOpts) -> Result<DeckPlan<'a>, String> {
    let found = search::find_by_name(cards, commander_name);
    let commander = found
        .iter()
        .find(|c| c.can_be_commander)
        .or(found.first())
        .copied()
        .ok_or_else(|| format!("carta não encontrada: {commander_name}"))?;
    if !commander.can_be_commander {
        return Err(format!("{} não pode ser comandante", commander.name));
    }
    let identity = &commander.color_identity;

    // sinergia: o texto do comandante vira a consulta
    let synergy = index.bm25.scores(&index.tok.tokenize(&format!("{} {}", commander.oracle_text, commander.tribes.join(" "))));
    let max_syn = synergy.iter().cloned().fold(0.0, f64::max).max(1e-9);

    // (nota combinada, sinergia pura, carta)
    let pool: Vec<(f64, f64, &Card)> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.oracle_id != commander.oracle_id
                && c.is_legal("commander")
                && c.color_identity.iter().all(|x| identity.contains(x))
                && !c.funny
                && o.budget.is_none_or(|b| c.price_usd.is_some_and(|p| p <= b))
                && (!o.no_game_changers || !c.game_changer)
        })
        .map(|(i, c)| {
            let syn = synergy[i] / max_syn;
            (0.6 * syn + 0.4 * popularity(c), syn, c)
        })
        .collect();

    let rank_by = |pred: &dyn Fn(&Card) -> bool, pure_synergy: bool| -> Vec<(f64, &'a Card)> {
        let mut v: Vec<(f64, &Card)> = pool
            .iter()
            .filter(|(_, _, c)| pred(c))
            .map(|(comb, syn, c)| (if pure_synergy { *syn } else { *comb }, *c))
            .collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v.truncate(o.per);
        v
    };
    let pick = |pred: &dyn Fn(&Card) -> bool| rank_by(pred, false);
    let non_land = |c: &Card| !c.types.iter().any(|t| t == "Land");

    let mut sections = vec![];
    let mut keys: Vec<(String, String)> = BASE.iter().map(|(k, t)| (k.to_string(), t.to_string())).collect();
    if identity.iter().any(|c| c == "U") {
        keys.push(("counterspell".into(), "Anulações".into()));
    }
    // temas do comandante viram seções próprias
    for t in &commander.tags.theme {
        keys.push((format!("theme:{t}"), format!("Tema do comandante: {t}")));
    }
    for t in &o.extra_tags {
        keys.push((t.clone(), format!("Extra: {t}")));
    }
    let mut seen = HashSet::new();
    for (key, title) in keys {
        if !seen.insert(key.clone()) {
            continue;
        }
        let cards = if key == "theme:tribal" {
            let tribes: Vec<&String> = commander.tribes.iter().chain(&commander.subtypes).collect();
            pick(&|c: &Card| non_land(c) && c.tribes.iter().chain(&c.subtypes).any(|t| tribes.contains(&t)))
        } else {
            pick(&|c: &Card| non_land(c) && c.tags.contains(&key))
        };
        sections.push(Section { key, title, cards });
    }

    sections.push(Section {
        key: "synergy".into(),
        title: "Maior sinergia com o texto do comandante".into(),
        cards: rank_by(&|c: &Card| non_land(c), true),
    });
    if identity.len() >= 2 {
        sections.push(Section {
            key: "lands".into(),
            title: "Terrenos de correção de cor".into(),
            cards: pick(&|c: &Card| !non_land(c) && c.tags.contains("fixing") && land_fits(c, identity)),
        });
    }

    Ok(DeckPlan { commander, sections })
}

/// O terreno só serve se toda a mana que gera/busca é das cores do comandante.
/// Ex.: para Golgari, Polluted Delta ("Island or Swamp") não entra; Overgrown Tomb entra.
fn land_fits(c: &Card, identity: &[String]) -> bool {
    let ok_color = |m: &str| m == "C" || identity.iter().any(|x| x == m);
    if !c.produced_mana.iter().all(|m| ok_color(m)) {
        return false;
    }
    let text = c.oracle_text.to_lowercase();
    let basics = [("plains", "W"), ("island", "U"), ("swamp", "B"), ("mountain", "R"), ("forest", "G")];
    let mentioned: Vec<&str> = basics.iter().filter(|(w, _)| text.contains(w)).map(|(_, col)| *col).collect();
    mentioned.iter().all(|m| ok_color(m))
}

#[cfg(test)]
mod tests {
    use super::land_fits;
    use crate::card::{Card, Tags};

    fn land(name: &str, text: &str, mana: &[&str]) -> Card {
        Card {
            oracle_id: String::new(), name: name.into(), layout: "normal".into(), mana_cost: String::new(), cmc: 0.0,
            type_line: "Land".into(), supertypes: vec![], types: vec!["Land".into()], subtypes: vec![],
            colors: vec![], color_identity: vec![], keywords: vec![], power: None, toughness: None,
            loyalty: None, defense: None, produced_mana: mana.iter().map(|s| s.to_string()).collect(),
            rarity: "rare".into(), set: String::new(), set_name: String::new(), released_at: String::new(),
            legal_formats: vec![], restricted_formats: vec![], edhrec_rank: None, game_changer: false, reserved: false,
            digital_only: false, funny: false, can_be_commander: false, instant_speed: false,
            price_usd: None, price_eur: None, oracle_text: text.into(), tags: Tags::default(), tribes: vec![],
            scryfall_uri: None, image: None, search_text: String::new(),
        }
    }

    #[test]
    fn off_color_fetchlands_are_rejected() {
        let bg = vec!["B".to_string(), "G".to_string()];
        let delta = land("Polluted Delta", "{T}, Pay 1 life, Sacrifice this land: Search your library for an Island or Swamp card, put it onto the battlefield, then shuffle.", &[]);
        let catacombs = land("Verdant Catacombs", "{T}, Pay 1 life, Sacrifice this land: Search your library for a Swamp or Forest card, put it onto the battlefield, then shuffle.", &[]);
        let tomb = land("Overgrown Tomb", "({T}: Add {B} or {G}.)\nAs this land enters, you may pay 2 life. If you don't, it enters tapped.", &["B", "G"]);
        let tundra = land("Tundra", "({T}: Add {W} or {U}.)", &["W", "U"]);
        assert!(!land_fits(&delta, &bg));
        assert!(land_fits(&catacombs, &bg));
        assert!(land_fits(&tomb, &bg));
        assert!(!land_fits(&tundra, &bg));
    }
}
