//! Estruturas de dados: a carta "crua" (bulk do Scryfall) e a carta final do dataset.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// Uma face de carta com mais de um lado (transform, MDFC, split, adventure...).
#[derive(Debug, Deserialize)]
pub struct RawFace {
    pub name: String,
    #[serde(default)]
    pub mana_cost: String,
    #[serde(default)]
    pub type_line: String,
    #[serde(default)]
    pub oracle_text: String,
    pub power: Option<String>,
    pub toughness: Option<String>,
    pub loyalty: Option<String>,
    pub defense: Option<String>,
    pub oracle_id: Option<String>,
    pub image_uris: Option<HashMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawPrices {
    pub usd: Option<String>,
    pub eur: Option<String>,
}

/// Formato do bulk "oracle-cards" do Scryfall. Campos que não usamos são ignorados.
#[derive(Debug, Deserialize)]
pub struct RawCard {
    pub oracle_id: Option<String>,
    pub name: String,
    pub layout: String,
    pub mana_cost: Option<String>,
    #[serde(default)]
    pub cmc: f64,
    pub type_line: Option<String>,
    pub oracle_text: Option<String>,
    pub card_faces: Option<Vec<RawFace>>,
    #[serde(default)]
    pub colors: Option<Vec<String>>,
    #[serde(default)]
    pub color_identity: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub power: Option<String>,
    pub toughness: Option<String>,
    pub loyalty: Option<String>,
    pub defense: Option<String>,
    pub produced_mana: Option<Vec<String>>,
    pub rarity: String,
    #[serde(default)]
    pub legalities: HashMap<String, String>,
    pub edhrec_rank: Option<u32>,
    #[serde(default)]
    pub game_changer: bool,
    #[serde(default)]
    pub reserved: bool,
    #[serde(default)]
    pub games: Vec<String>,
    pub set: String,
    pub set_name: String,
    pub set_type: String,
    pub released_at: String,
    pub scryfall_uri: Option<String>,
    pub image_uris: Option<HashMap<String, String>>,
    #[serde(default)]
    pub prices: RawPrices,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tags {
    /// Função no deck: removal, ramp, card_draw...
    pub function: BTreeSet<String>,
    /// Quando a carta faz algo: etb, dies, attacks...
    pub trigger: BTreeSet<String>,
    /// O que ela mira: creature, player, spell...
    pub target: BTreeSet<String>,
    /// Arquétipo/estratégia de deck em que se encaixa: aristocrats, tokens...
    pub theme: BTreeSet<String>,
}

impl Tags {
    /// Aceita "removal" (qualquer categoria) ou "categoria:tag" ("theme:lifegain").
    pub fn contains(&self, t: &str) -> bool {
        let t = t.to_lowercase();
        match t.split_once(':') {
            Some((cat, tag)) => self.category(cat).is_some_and(|s| s.contains(tag)),
            None => self.all().any(|x| *x == t),
        }
    }

    pub fn category(&self, cat: &str) -> Option<&BTreeSet<String>> {
        match cat {
            "function" | "funcao" | "função" => Some(&self.function),
            "trigger" | "gatilho" => Some(&self.trigger),
            "target" | "alvo" => Some(&self.target),
            "theme" | "tema" => Some(&self.theme),
            _ => None,
        }
    }

    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.function.iter().chain(&self.trigger).chain(&self.target).chain(&self.theme)
    }
}

/// Formato de data/cards.json: o dataset que a busca usa.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub oracle_id: String,
    pub name: String,
    pub layout: String,
    pub mana_cost: String,
    pub cmc: f64,
    pub type_line: String,
    pub supertypes: Vec<String>,
    pub types: Vec<String>,
    pub subtypes: Vec<String>,
    pub colors: Vec<String>,
    pub color_identity: Vec<String>,
    pub keywords: Vec<String>,
    pub power: Option<String>,
    pub toughness: Option<String>,
    pub loyalty: Option<String>,
    pub defense: Option<String>,
    pub produced_mana: Vec<String>,
    pub rarity: String,
    pub set: String,
    pub set_name: String,
    pub released_at: String,
    pub legal_formats: Vec<String>,
    /// formatos em que a carta é restrita (máx. 1 cópia), ex. Black Lotus no vintage.
    /// Também aparecem em legal_formats.
    #[serde(default)]
    pub restricted_formats: Vec<String>,
    pub edhrec_rank: Option<u32>,
    pub game_changer: bool,
    pub reserved: bool,
    /// só existe no digital (Arena/MTGO)
    pub digital_only: bool,
    /// cartas "Un-" / acorn, sem valor em torneio
    pub funny: bool,
    pub can_be_commander: bool,
    /// instantâneo ou com Flash
    pub instant_speed: bool,
    pub price_usd: Option<f64>,
    pub price_eur: Option<f64>,
    pub oracle_text: String,
    pub tags: Tags,
    /// tipos de criatura que a carta menciona no texto (sinergia tribal)
    pub tribes: Vec<String>,
    pub scryfall_uri: Option<String>,
    pub image: Option<String>,
    pub search_text: String,
}

impl Card {
    pub fn is_legal(&self, format: &str) -> bool {
        let f = format.to_lowercase();
        self.legal_formats.contains(&f)
    }

    pub fn has_type(&self, t: &str) -> bool {
        let t = t.to_lowercase();
        self.type_line.to_lowercase().contains(&t)
    }
}
