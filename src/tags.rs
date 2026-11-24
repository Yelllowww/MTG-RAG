//! Classificação das cartas em 4 categorias, pensando em busca e montagem de deck:
//!
//! * **function** – o papel da carta no deck (removal, ramp, card_draw...)
//! * **trigger**  – quando ela age (etb, dies, attacks...)
//! * **target**   – o que ela mira (creature, player, spell...)
//! * **theme**    – arquétipos/estratégias em que se encaixa (aristocrats, tokens...)
//!
//! As regras rodam sobre o texto oracle "normalizado": sem texto de lembrete
//! (entre parênteses), em minúsculas, com o nome da carta e "this creature"
//! trocados por `~`, e "creature card" virando `creature_card` (para não
//! confundir "exile target creature" com "exile target creature card from a graveyard").
//!
//! Regras simples ficam nas tabelas `*_RULES` (regex). O que regex não resolve
//! bem (tutor x busca de terreno, recursão da própria carta, mana) está em
//! `Classifier::classify`.

use crate::card::Tags;
use regex::Regex;
use std::collections::{BTreeSet, HashMap};

/// (tag, padrões, ignorar se a oração contiver algum destes trechos)
pub type Rule = (&'static str, &'static [&'static str], &'static [&'static str]);

// ---------------------------------------------------------------- taxonomia

/// Todas as tags, por categoria, com rótulo em português (vai para o texto de busca).
pub const TAXONOMY: &[(&str, &[(&str, &str)])] = &[
    ("function", &[
        ("removal", "remoção pontual, remover criatura ou permanente"),
        ("board_wipe", "board wipe, limpa mesa, remoção em massa"),
        ("counterspell", "anular mágica, counter"),
        ("card_draw", "compra de cartas, card advantage"),
        ("card_selection", "filtrar cartas, scry, surveil, looting"),
        ("impulse_draw", "compra por exílio, jogar do topo do grimório"),
        ("wheel", "wheel, todos descartam e compram"),
        ("tutor", "tutor, buscar carta no grimório"),
        ("ramp", "ramp, aceleração de mana"),
        ("mana_rock", "pedra de mana, artefato que gera mana"),
        ("mana_dork", "criatura que gera mana"),
        ("ritual", "ritual, mana temporária"),
        ("land_ramp", "buscar terreno para o campo"),
        ("extra_land", "terreno adicional por turno"),
        ("fixing", "correção de cores, mana de várias cores"),
        ("treasure", "tesouro, fichas de tesouro"),
        ("cost_reducer", "reduzir custo"),
        ("token_maker", "criar fichas de criatura, tokens"),
        ("copy", "copiar mágica ou permanente"),
        ("doubler", "dobrar fichas ou marcadores"),
        ("trigger_doubler", "dobrar gatilhos"),
        ("recursion", "recursão, voltar do cemitério para a mão"),
        ("reanimation", "reanimar, do cemitério para o campo"),
        ("graveyard_hate", "exilar cemitério, ódio ao cemitério"),
        ("self_mill", "moer o próprio grimório"),
        ("mill", "mill, moer grimório do oponente"),
        ("discard", "descarte forçado do oponente"),
        ("protection", "proteção, indestrutível, hexproof"),
        ("lifegain", "ganhar vida"),
        ("drain", "drenar, oponentes perdem vida"),
        ("burn", "dano direto ao jogador"),
        ("bounce", "devolver para a mão"),
        ("blink", "blink, flicker, exilar e devolver"),
        ("theft", "roubar, ganhar controle"),
        ("tax_stax", "taxa, stax, travar oponente"),
        ("land_destruction", "destruir terrenos"),
        ("pump", "aumentar poder, buff"),
        ("anthem", "buff para o time todo, anthem"),
        ("plus_counters", "marcadores +1/+1"),
        ("proliferate", "proliferar"),
        ("haste_enabler", "conceder ímpeto, haste"),
        ("evasion", "evasão, voar, atropelar, imbloqueável"),
        ("sacrifice_outlet", "sacrificar, outlet de sacrifício"),
        ("untap", "desvirar"),
        ("extra_turn", "turno extra"),
        ("extra_combat", "combate extra"),
        ("cheat", "colocar no campo sem pagar"),
        ("fog", "prevenir dano de combate, fog"),
        ("alt_win", "vitória alternativa, wincon"),
        ("poison", "veneno, infectar"),
        ("fling", "fling, dano ou perda de vida igual ao poder da criatura sacrificada"),
        ("death_value", "valor ao morrer, fodder de sacrifício"),
        ("recurring", "volta sozinha do cemitério, undying, persist, dredge"),
        ("discard_outlet", "descartar como custo, outlet de descarte"),
        ("edict", "edito, oponente sacrifica criatura"),
        ("hand_disruption", "atacar a mão do oponente, ver a mão"),
        ("free_spell", "mágica grátis, custo alternativo"),
        ("scaling", "poder escala, fica maior por contagem"),
    ]),
    ("trigger", &[
        ("etb", "ao entrar no campo"),
        ("ltb", "ao sair do campo"),
        ("dies", "quando morre"),
        ("attacks", "ao atacar"),
        ("blocks", "ao bloquear"),
        ("combat_damage", "ao causar dano de combate"),
        ("cast_spell", "ao conjurar mágica"),
        ("upkeep", "na manutenção"),
        ("begin_combat", "no início do combate"),
        ("end_step", "na etapa final"),
        ("draw_trigger", "quando alguém compra carta"),
        ("landfall", "landfall, quando terreno entra"),
        ("lifegain_trigger", "quando ganha vida"),
        ("sacrifice_trigger", "quando sacrifica"),
        ("counter_trigger", "quando recebe marcador"),
        ("discard_trigger", "quando descarta"),
        ("token_trigger", "quando cria ficha"),
        ("activated", "habilidade ativada"),
    ]),
    ("target", &[
        ("creature", "alvo criatura"),
        ("artifact", "alvo artefato"),
        ("enchantment", "alvo encantamento"),
        ("planeswalker", "alvo planeswalker"),
        ("land", "alvo terreno"),
        ("battle", "alvo batalha"),
        ("permanent", "alvo permanente"),
        ("player", "alvo jogador"),
        ("spell", "alvo mágica"),
        ("ability", "alvo habilidade"),
        ("any_target", "qualquer alvo"),
        ("graveyard_card", "alvo carta no cemitério"),
    ]),
    ("theme", &[
        ("aristocrats", "aristocratas, sacrifício, morte de criaturas"),
        ("tokens", "fichas, go wide"),
        ("counters", "marcadores +1/+1, proliferar"),
        ("spellslinger", "mágicas instantâneas e feitiços"),
        ("artifacts", "artefatos importam"),
        ("enchantress", "encantamentos importam"),
        ("voltron", "equipamentos e auras, voltron"),
        ("reanimator", "reanimação"),
        ("graveyard", "cemitério importa"),
        ("lands", "terrenos importam, landfall"),
        ("lifegain", "ganho de vida importa"),
        ("mill", "mill"),
        ("blink", "blink, flicker"),
        ("stax", "stax, controle de recursos"),
        ("group_hug", "group hug, ajuda todos os jogadores"),
        ("superfriends", "planeswalkers, superfriends"),
        ("storm", "storm, muitas mágicas por turno"),
        ("wheels", "wheels, compra em massa"),
        ("treasure", "tesouros"),
        ("vehicles", "veículos"),
        ("tribal", "tribal, tipo de criatura"),
        ("poison", "veneno"),
        ("discard", "descarte, madness"),
        ("energy", "energia"),
    ]),
];

pub fn pt_label(tag: &str) -> Option<&'static str> {
    TAXONOMY.iter().flat_map(|(_, t)| t.iter()).find(|(n, _)| *n == tag).map(|(_, l)| *l)
}

/// Categoria de uma tag. Se o nome existir em mais de uma (ex.: "lifegain"),
/// devolve a primeira — use "theme:lifegain" para ser explícito.
pub fn category_of(tag: &str) -> Option<&'static str> {
    TAXONOMY.iter().find(|(_, t)| t.iter().any(|(n, _)| *n == tag)).map(|(c, _)| *c)
}

// ---------------------------------------------------------------- regras

pub const FUNCTION_RULES: &[Rule] = &[
    ("removal", &[
        r"\b(destroy|exile) (up to (one|two|three|four|x|\w+) )?(another |other )?target ([\w-]+ ){0,3}?(creatures?|permanents?|artifacts?|enchantments?|planeswalkers?|battles?)\b",
        r"\btarget ([\w-]+ ){0,3}?creature ([^.]*? )?gets -(\d+|x)/-(\d+|x)",
        r"\bdeals? [^.]*?damage[^.]*? to (any target|(up to \w+ )?(another |other )?target ([\w-]+ ){0,2}?(creatures?|planeswalkers?|battles?))\b",
        r"\bdamage divided as you choose among [^.]*targets",
        r"\bfights? (up to one )?(another |other )?target",
        r"\bput target [^.]* (on top of|on the bottom of|into) (its|their) owners?'? librar",
        r"\btarget [^.]*shuffles? it into",
        r"\bowner of target [^.]*shuffles",
        r"\b(target|each) (player|opponent) sacrifices (a|an|one|two|\w+) ",
    ], &["you control", "you own"]),
    ("board_wipe", &[
        r"\b(destroy|exile) (all|each) ([\w-]+ ){0,4}?(creatures?|permanents?|artifacts?|enchantments?|planeswalkers?|lands?)\b",
        r"\b(all|each) (other )?([\w-]+ ){0,2}?creatures?( [^.]*?)? gets? -(\d+|x)/-(\d+|x)",
        r"\bdeals? [^.]*?damage to each (other )?([\w-]+ ){0,2}?creature",
        r"\breturn all [^.]*(permanents|creatures|artifacts|enchantments)[^.]* to their owners?'? hands?",
        r"\b(each player|all players) sacrifices? (all|each)",
        r"\bsacrifices? all (creatures|permanents|nonland)",
    ], &["you control", "blocking", "blocked by", "it blocks"]),
    ("counterspell", &[
        r"\bcounter (up to \w+ )?(target|all|each|another target) ([\w-]+ ){0,4}?(spells?|abilit(y|ies))\b",
        r"\bcounter (that|the chosen|those|the first) (spells?|abilit)",
        r"\bcounter it\b",
    ], &[]),
    ("card_draw", &[
        r"\bdraws? (a|an|one|two|three|four|five|six|seven|eight|x|that many|\w+) (additional )?cards?\b",
        r"\bdraws? cards? equal",
        r"\bdraw cards\b",
        r"\binvestigate\b",
        r"\bcreates? [^.]*\bclue tokens?",
        r"\bbecomes? the monarch\b",
        r"\byou become the monarch\b",
    ], &["whenever an opponent draws", "whenever a player draws", "whenever each opponent draws", "whenever another player draws"]),
    ("card_selection", &[
        r"\bscry (\d+|x)\b",
        r"\bsurveil (\d+|x)\b",
        r"\blook at the top [^.]*of your library",
        r"\bdraws? [^.]*,? then discards?",
        r"\bdiscards? [^.]*(, then|then|. if you do,) draws?",
        r"\bconnives?\b",
    ], &[]),
    ("impulse_draw", &[
        r"\bexile the top [^\n]*?librar(y|ies)[^\n]*?\byou may (play|cast)",
        r"\b(play|cast) [^.]*from the top of your library",
    ], &[]),
    ("wheel", &[
        r"\bdraws? seven cards",
        r"\beach player (discards|shuffles) [^.]*hand[^.]*draws",
    ], &[]),
    ("extra_land", &[
        r"\bplay (an|two|three) additional lands?",
        r"\bput (a|up to \w+|any number of) land_cards? from your hand onto the battlefield",
    ], &[]),
    ("treasure", &[r"\bcreates? [^.]*\btreasure tokens?"], &[]),
    ("cost_reducer", &[
        r"\b(spells?|abilit(y|ies))\b[^.~]{0,80}?\bcosts? (\{[^}]+\})+ less",
    ], &[]),
    ("token_maker", &[
        r"\bcreates? [^.]*?creature tokens?",
        r"\bcreates? [^.]*?tokens? that'?s? a copy",
        r"\bpopulate\b",
        r"\bamass\b",
        r"\bincubate\b",
    ], &[]),
    ("copy", &[
        r"\bcopy (target|that|it|the next|each|those)\b",
        r"\bcreates? [^.]*tokens? that'?s? a copy",
        r"\b(enter|enters) as a copy",
        r"\bbecomes? a copy",
    ], &[]),
    ("doubler", &[r"\btwice that many\b", r"\bdouble the (number|amount)", r"\bthat many plus one"], &[]),
    ("trigger_doubler", &[r"\btriggers? an additional time"], &[]),
    ("graveyard_hate", &[
        r"\bexile [^.]*\b(target player's|each opponent's|an opponent's|all|each player's|target opponent's|that player's) graveyards?",
        r"\bexile (up to \w+ )?target [^.]*cards? (from|in) (a|an opponent's|target player's) graveyard",
        r"\bexile [^.]*\bcards? from (a|all|target player's|each opponent's|an opponent's) graveyards?",
        r"\bcards? in graveyards can't",
        r"\bwould be put into (an opponent's|a) graveyard[^.]*exile it instead",
        r"\bshuffles? (their|his or her|target player's) graveyard into",
    ], &[]),
    ("mill", &[
        r"\b(target (player|opponent)|each (opponent|player)|that player|defending player|target players?|any number of target players) mills?\b",
        r"\bput the top [^.]*of (target|each|that) (player's|opponent's) librar(y|ies) into",
    ], &[]),
    ("self_mill", &[
        r"(^|[\n.:,] |then |you may |\n)mill (a|one|two|three|four|five|six|seven|x|that many|\w+) cards?",
        r"\bput the top [^.]*of your library into your graveyard",
        r"\bsurveil\b",
        r"\bput (that card|it|them|those cards) into your graveyard",
    ], &[]),
    ("discard", &[
        r"\b(target (player|opponent)|each (opponent|player)|that player|defending player) (discards|reveals (their|his or her) hand)",
        r"\btarget (player|opponent) reveals",
    ], &[]),
    ("protection", &[
        r"\b(gains?|gain|have|has|gets?) [^.]*?\b(hexproof|indestructible|shroud|protection from|ward)\b",
        r"\bphases? out",
        r"\bcan't be (countered|the targets? of)",
        r"\bprevent all damage that would be dealt to (target|you|creatures you control|permanents you control)",
    ], &["~ gains", "~ has", "~ can't", "~ gets"]),
    ("lifegain", &[
        r"\byou gain (\d+|x|that much|\w+) life",
        r"\byou gain life equal",
        r"(^|[\n.:,] |then )gain (\d+|x|\w+) life",
        r"\b(gains?|have|has) [^.]*\blifelink",
        r"\bcreates? [^.]*\bfood tokens?",
    ], &[]),
    ("drain", &[
        r"\b(each opponent|target (player|opponent)|each player|that player|defending player|the chosen player) loses (\d+|x|\w+|life|half)",
        r"\bdeals? [^.]*damage to each opponent",
    ], &[]),
    ("burn", &[
        r"\bdeals? [^.]*?damage[^.]*? to (any target|any one target|target (player|opponent)|each opponent|each player|target player or planeswalker|that player|defending player)",
        r"\bdamage divided as you choose among [^.]*targets",
    ], &[]),
    ("bounce", &[
        r"\breturn (target|up to (one|two|three|\w+) target|another target|all|each|x target) [^.]*\bto (its|their) owners?'?s?'? hands?",
    ], &["you control", "you own"]),
    ("blink", &[
        r"\bexile [^\n]*?\breturn (it|that card|those cards|them|the exiled cards?|each card exiled|that permanent)[^.\n]* to the battlefield",
    ], &[]),
    ("theft", &[r"\bgain control of\b", r"\bexchange control\b"], &[]),
    ("tax_stax", &[
        r"\b(spells?|abilit(y|ies))\b[^.~]{0,80}?\bcosts? (\{[^}]+\})+ more",
        r"\bunless (its controller|that player|they|he or she) pays?",
        r"\b(don't|doesn't|can't) untap",
        r"\b(your opponents|each opponent|opponents|players|each player|that player|target player|target opponent)( [^.]*?)? can't (cast|attack|activate|search|draw|play|gain|untap)",
        r"\bcan't attack you\b",
        r"\b(your opponents|opponents) control enter (the battlefield )?tapped",
        r"\bskips? (their|your|his or her) (untap|draw|next)",
    ], &["counter target", "counter that", "~ doesn't", "~ don't"]),
    ("land_destruction", &[
        r"\bdestroy (target|all|each|up to \w+ target|x target|two target) ([\w-]+ ){0,2}?lands?\b",
        r"\b(each player|target player|each opponent) sacrifices [^.]*\blands?\b",
    ], &[]),
    ("pump", &[r"\bgets? \+(\d+|x)/\+(\d+|x)"], &["~ gets"]),
    // anthem = o time todo: verbo no plural ("creatures you control get") ou "each creature you control gets".
    // "target creature you control gets +1/+0" é só pump.
    ("anthem", &[
        r"\byou control\b[^.]*?\bget \+(\d+|x)/\+(\d+|x)",
        r"\beach (other )?([\w-]+ ){0,2}?creature you control gets \+(\d+|x)/\+(\d+|x)",
    ], &[]),
    ("plus_counters", &[r"\+1/\+1 counters?"], &[]),
    ("proliferate", &[r"\bproliferate\b"], &[]),
    ("haste_enabler", &[r"\b(gains?|gain|have|has) [^.]*?\bhaste\b", r"\bas though (it|they) had haste"], &["~ has", "~ gains"]),
    ("evasion", &[
        r"\b(gains?|gain|have|has) [^.]*?\b(flying|trample|menace|shadow|fear|intimidate|skulk|horsemanship)\b",
        r"\bcan't be blocked\b",
    ], &["~ can't be blocked", "~ gains", "~ has"]),
    ("sacrifice_outlet", &[
        r"\bsacrifice (a|an|another|one or more|any number of|x|two|\w+) ([\w-]+ ){0,2}?(creatures?|artifacts?|permanents?|tokens?|lands?)\b[^.:\n]*:",
        r"\badditional cost[^.]*\bsacrifice (a|an|another|one or more|x) ",
    ], &[]),
    ("untap", &[r"\buntap (target|all|each|up to|another|those|that|them|it|x|two)\b"], &[]),
    ("extra_turn", &[r"\bextra turn\b"], &["can't take"]),
    ("extra_combat", &[r"\badditional combat phase\b"], &[]),
    ("cheat", &[
        r"\bput [^.]*_cards? from your hand onto the battlefield",
        r"\bput [^.]*_cards? [^.]*from (among them|your library)[^.]*onto the battlefield",
    ], &["land_card"]),
    ("fog", &[r"\bprevent all combat damage\b"], &[]),
    ("alt_win", &[r"\byou win the game\b", r"\b(target|that|each) (player|opponent)s? loses? the game\b"], &["can't win", "can't lose"]),
    ("poison", &[r"\bpoison counters?\b", r"\btoxic\b", r"\binfect\b"], &[]),
    ("fling", &[r"\b(damage|loses? life|life) equal to the sacrificed (creature|permanent|artifact)'s (power|toughness)"], &[]),
    // só a própria carta morrendo (fodder); "~ or another creature dies" é payoff (Blood Artist)
    ("death_value", &[r"\bwhen(ever)? ~ (dies|is put into a graveyard from the battlefield)"], &[]),
    ("discard_outlet", &[
        r"\bdiscard (a|an|one|two|three|x|\w+) ([\w-]+ )?\w*cards?\b[^.:\n]*:",
        r"\badditional cost[^.]*\bdiscard (a|an|one|two|x) ",
    ], &[]),
    ("edict", &[
        r"\b(target|each|that) (player|opponent)s? sacrifices? (a|an|one|two|three|x|\w+) ([\w-]+ ){0,2}?(creatures?|permanents?|planeswalkers?|artifacts?|enchantments?)\b",
    ], &["you control"]),
    ("hand_disruption", &[
        r"\b(target|each) (player|opponent)( [^.]*?)? reveals? (their|his or her) hand",
        r"\bputs? a card from (their|his or her) hand on top of (their|his or her) library",
        r"\bexile [^.]*from (their|target player's|target opponent's|that player's|an opponent's|each opponent's) hand",
        r"\blook at (target|each) (player's|opponent's) hand",
    ], &[]),
    ("free_spell", &[r"\brather than pay (~'s|its|this spell's) mana cost"], &[]),
    ("scaling", &[
        r"~'s power (and toughness )?(are each|is) equal to",
        // sem \b antes de "~": "~" não é caractere de palavra, então \b nunca casa ali
        r"~ gets \+\d+/\+\d+ for each",
        r"~ gets \+x/\+x\b",
    ], &[]),
];

pub const TRIGGER_RULES: &[Rule] = &[
    ("etb", &[r"\bwhen(ever)? [^.]*?\b(enters|enter)\b"], &[]),
    ("ltb", &[r"\bwhen(ever)? [^.]*?\bleaves? the battlefield"], &[]),
    ("dies", &[
        r"\bwhen(ever)? [^.]*?\b(dies|die)\b",
        r"\b(is|are) put into (a|your|an opponent's|its owner's|their owners'?) graveyards? from the battlefield",
    ], &[]),
    ("attacks", &[r"\bwhen(ever)? [^.]*?\b(attacks|attack)\b"], &[]),
    ("blocks", &[r"\bwhen(ever)? [^.]*?\b(blocks|becomes blocked)\b"], &[]),
    ("combat_damage", &[r"\bwhen(ever)? [^.]*?\bdeals? combat damage\b"], &[]),
    ("cast_spell", &[r"\bwhenever (you|an opponent|a player|another player|each opponent|target opponent) casts?\b"], &[]),
    ("upkeep", &[r"\bat the beginning of (your|each|each player's|each opponent's|the next|target player's|that player's|enchanted player's) upkeep"], &[]),
    ("begin_combat", &[r"\bat the beginning of (combat|each combat|your combat)"], &[]),
    ("end_step", &[r"\bat the beginning of (your|each|each player's|each opponent's|the next|the) end step"], &[]),
    ("draw_trigger", &[r"\bwhenever (you|a player|an opponent|each opponent|target opponent) draws?\b"], &[]),
    ("landfall", &[r"\bwhenever (a|one or more|another) (\w+ )?lands? (you control )?(enters|enter)", r"\blandfall\b"], &[]),
    ("lifegain_trigger", &[r"\bwhenever you gain life", r"\bwhenever [^.]*\bgains? life\b"], &[]),
    ("sacrifice_trigger", &[r"\bwhenever (you|a player|an opponent) sacrifices?\b"], &[]),
    ("counter_trigger", &[r"\bwhenever (one or more )?[^.]*counters? (is|are) put on", r"\bwhenever you put (one or more )?[^.]*counters? on"], &[]),
    ("discard_trigger", &[r"\bwhenever (you|a player|an opponent|each opponent) discards?\b"], &[]),
    ("token_trigger", &[r"\bwhenever (you create|one or more tokens?|a token)"], &[]),
];

pub const TARGET_RULES: &[Rule] = &[
    ("creature", &[r"\btarget ([\w-]+ ){0,3}?creatures?\b"], &[]),
    ("artifact", &[r"\btarget ([\w-]+ ){0,3}?artifacts?\b"], &[]),
    ("enchantment", &[r"\btarget ([\w-]+ ){0,3}?enchantments?\b"], &[]),
    ("planeswalker", &[r"\btarget ([\w-]+ ){0,3}?planeswalkers?\b"], &[]),
    ("land", &[r"\btarget ([\w-]+ ){0,3}?lands?\b"], &[]),
    ("battle", &[r"\btarget ([\w-]+ ){0,3}?battles?\b"], &[]),
    ("permanent", &[r"\btarget ([\w-]+ ){0,3}?permanents?\b"], &[]),
    ("player", &[r"\btarget (player|opponent|players|opponents)\b"], &[]),
    ("spell", &[r"\btarget ([\w-]+ ){0,3}?spells?\b"], &[]),
    ("ability", &[r"\btarget ([\w-]+ ){0,3}?abilit(y|ies)\b"], &[]),
    ("any_target", &[r"\bany (one )?target\b", r"\bany number of targets\b"], &[]),
    ("graveyard_card", &[r"\btarget ([\w-]+ ){0,3}?\w*_?cards? (from|in) (a|your|an opponent's|target player's|their) graveyard", r"\btarget [\w ]{0,30}?\w+_cards?\b"], &[]),
];

/// Parte dos temas detectada por texto; o resto vem das outras tags (em `classify`).
pub const THEME_RULES: &[Rule] = &[
    ("aristocrats", &[
        r"\b(another|a|one or more|nontoken) ([\w-]+ )?creatures?( you control)? (dies|die)\b",
        r"\bor another creature (you control )?dies",
        r"\bwhenever you sacrifice",
    ], &[]),
    ("tokens", &[r"\btokens? you control", r"\bfor each token", r"\bwhenever (you create|one or more tokens?|a token)", r"\bpopulate\b"], &[]),
    ("counters", &[r"\+1/\+1 counters?", r"\bproliferate\b", r"\bcounters? (is|are) put on"], &[]),
    ("spellslinger", &[
        r"\binstant (and|or) sorcery",
        r"\bwhenever you cast (an? )?(instant|sorcery|noncreature)",
        r"\bmagecraft\b",
        r"\bprowess\b",
        r"\bnoncreature spells?",
    ], &[]),
    ("artifacts", &[
        r"\bartifacts? you control",
        r"\bfor each artifact",
        r"\bwhenever (an?|another|one or more) (\w+ )?artifacts? (you control )?(enters|is put)",
        r"\bartifact spells?",
        r"\bmetalcraft\b",
        r"\baffinity for artifacts",
        r"\bimprovise\b",
    ], &[]),
    ("enchantress", &[
        r"\benchantments? you control",
        r"\benchantment spells?",
        r"\bwhenever (an?|another|one or more) (\w+ )?enchantments? (you control )?enters",
        r"\bconstellation\b",
        r"\bfor each enchantment",
    ], &[]),
    ("voltron", &[
        r"\bequipped creature",
        r"\benchanted creature (gets \+|has|gains)",
        r"\bequipment you control",
        r"\bauras? you control",
        r"\battach",
    ], &[]),
    ("reanimator", &[r"\bcreature_cards? in your graveyard", r"\bcreature_card in a graveyard"], &[]),
    ("graveyard", &[
        r"\bfrom your graveyard", r"\bcards? in your graveyard", r"_cards? in your graveyard",
        r"\bcards? in (all|each|a) graveyards?", r"\bthreshold\b", r"\bdelirium\b",
    ], &[]),
    ("lands", &[
        r"\blands? you control",
        r"\blandfall\b",
        r"\bwhenever (a|one or more) lands? (you control )?enters",
        r"\bland_cards? (from|in) your graveyard",
        r"\bplay lands from",
    ], &[]),
    ("lifegain", &[r"\bwhenever you gain life", r"\bgained life this turn", r"\bfor each 1 life you gained", r"\bwhenever [^.]*\bgains? life\b"], &[]),
    ("group_hug", &[r"\beach player (draws|may draw|adds|may put)", r"\beach opponent (draws|may draw)"], &[]),
    ("superfriends", &[r"\bplaneswalkers? you control", r"\bloyalty (abilit|counter)", r"\bplaneswalker spells?"], &[]),
    ("storm", &[r"\bstorm\b", r"\bspells? (you('ve)? cast|cast) this turn", r"\bfor each other spell"], &[]),
    ("wheels", &[r"\bwhenever (an opponent|a player|each opponent) draws"], &[]),
    ("treasure", &[r"\btreasures? you control", r"\bsacrifice a treasure", r"\bwhenever [^.]*treasure"], &[]),
    ("vehicles", &[r"\bvehicles? you control", r"\bcrews?\b"], &[]),
    ("discard", &[r"\bwhenever you discard", r"\bmadness\b"], &[]),
    ("energy", &[r"\{e\}"], &[]),
];

/// Ajustes manuais para o que as regras erram: (nome, adicionar, remover).
/// Tags podem ser qualificadas ("theme:tokens") ou não (vai para a categoria da taxonomia).
pub const MANUAL: &[(&str, &[&str], &[&str])] = &[
    ("Seedborn Muse", &["ramp"], &[]),
    ("Snap", &["ramp"], &[]),
    ("Cyclonic Rift", &["removal", "board_wipe"], &[]),
    ("Aetherize", &[], &["board_wipe"]),
    ("Sol Ring", &["mana_rock", "ramp"], &[]),
];

pub fn color_pt(c: &str) -> &'static str {
    match c {
        "W" => "branca",
        "U" => "azul",
        "B" => "preta",
        "R" => "vermelha",
        "G" => "verde",
        _ => "?",
    }
}

pub fn type_pt(t: &str) -> Option<&'static str> {
    Some(match t {
        "Creature" => "criatura",
        "Instant" => "instantâneo, mágica instantânea",
        "Sorcery" => "feitiço",
        "Artifact" => "artefato",
        "Enchantment" => "encantamento",
        "Land" => "terreno",
        "Planeswalker" => "planeswalker",
        "Battle" => "batalha",
        "Kindred" | "Tribal" => "tribal",
        "Legendary" => "lendária",
        _ => return None,
    })
}

// ---------------------------------------------------------------- motor

struct Compiled(Vec<(&'static str, Vec<Regex>, &'static [&'static str])>);

impl Compiled {
    fn new(rules: &[Rule]) -> Self {
        Compiled(
            rules
                .iter()
                .map(|(tag, pats, skip)| {
                    let res = pats
                        .iter()
                        .map(|p| Regex::new(p).unwrap_or_else(|e| panic!("regex inválida em {tag}: {e}")))
                        .collect();
                    (*tag, res, *skip)
                })
                .collect(),
        )
    }

    fn tag(&self, text: &str, out: &mut BTreeSet<String>) {
        for (tag, res, skip) in &self.0 {
            let hit = res.iter().any(|r| {
                r.find_iter(text).any(|m| {
                    skip.is_empty() || {
                        let cl = clause(text, m.start(), m.end());
                        !skip.iter().any(|s| cl.contains(s))
                    }
                })
            });
            if hit {
                out.insert(tag.to_string());
            }
        }
    }
}

/// A oração (entre pontos/quebras de linha) que contém o trecho [s, e).
fn clause(text: &str, s: usize, e: usize) -> &str {
    let a = text[..s].rfind(['.', '\n']).map_or(0, |i| i + 1);
    let b = text[e..].find(['.', '\n']).map_or(text.len(), |i| e + i);
    &text[a..b]
}

/// Dados de uma carta necessários para classificá-la.
pub struct Input<'a> {
    /// nome completo e nome de cada face
    pub names: Vec<&'a str>,
    /// texto oracle (faces juntadas por quebra de linha)
    pub text: &'a str,
    pub types: &'a [String],
    pub subtypes: &'a [String],
    pub keywords: &'a [String],
    pub produced_mana: &'a [String],
}

pub struct Classifier {
    function: Compiled,
    trigger: Compiled,
    target: Compiled,
    theme: Compiled,
    reminder: Regex,
    this_obj: Regex,
    typed_card: Regex,
    search: Regex,
    gy_move: Regex,
    gy_cast: Regex,
    add_mana: Regex,
    extra_mana: Regex,
    activated: Regex,
    tribe: Option<Regex>,
    tribe_base: HashMap<String, String>,
    token_after: Regex,
}

/// Palavras que aparecem como subtipo de criatura mas causam falso positivo.
const TRIBE_BLACKLIST: &[&str] = &["The", "Time", "Lord", "Will", "Child", "Egg", "Wall", "Citizen", "Performer"];

fn plural_forms(t: &str) -> Vec<String> {
    let mut v = vec![t.to_string()];
    match t {
        "Elf" => v.push("Elves".into()),
        "Dwarf" => v.push("Dwarves".into()),
        "Wolf" => v.push("Wolves".into()),
        "Mouse" => v.push("Mice".into()),
        "Ally" => v.push("Allies".into()),
        "Fungus" => v.push("Fungi".into()),
        "Merfolk" | "Sheep" | "Fish" | "Kor" | "Moonfolk" | "Kithkin" | "Leviathan" => {}
        _ if t.ends_with('s') || t.ends_with('x') || t.ends_with("ch") || t.ends_with("sh") => {
            v.push(format!("{t}es"))
        }
        _ if t.ends_with('y') && !t.ends_with("ey") => v.push(format!("{}ies", &t[..t.len() - 1])),
        _ => v.push(format!("{t}s")),
    }
    v
}

impl Classifier {
    /// `creature_types`: todos os subtipos de criatura do dataset (para detectar tribal).
    pub fn new(creature_types: &BTreeSet<String>) -> Self {
        let mut tribe_base = HashMap::new();
        for t in creature_types {
            if t.len() < 3 || TRIBE_BLACKLIST.contains(&t.as_str()) || !t.chars().all(|c| c.is_alphabetic() || c == '-') {
                continue;
            }
            for f in plural_forms(t) {
                tribe_base.insert(f, t.clone());
            }
        }
        let tribe = if tribe_base.is_empty() {
            None
        } else {
            let mut forms: Vec<&String> = tribe_base.keys().collect();
            forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
            let alt: Vec<String> = forms.iter().map(|f| regex::escape(f)).collect();
            Some(Regex::new(&format!(r"\b({})\b", alt.join("|"))).unwrap())
        };

        Classifier {
            function: Compiled::new(FUNCTION_RULES),
            trigger: Compiled::new(TRIGGER_RULES),
            target: Compiled::new(TARGET_RULES),
            theme: Compiled::new(THEME_RULES),
            reminder: Regex::new(r" ?\([^)]*\)").unwrap(),
            this_obj: Regex::new(
                r"\bthis (creature|artifact|enchantment|land|planeswalker|spell|card|permanent|vehicle|equipment|aura|saga|battle|siege|class|case|room|token|object)\b",
            )
            .unwrap(),
            typed_card: Regex::new(
                r"\b(instant (?:and|or) sorcery|basic land|creature|artifact|enchantment|land|permanent|planeswalker|instant|sorcery|legendary|nonland|noncreature|historic|aura|equipment|vehicle|plains|island|swamp|mountain|forest) (cards?)\b",
            )
            .unwrap(),
            search: Regex::new(r"\bsearch (your|their|target player's|its owner's|that player's|his or her) library for ([^.]*)").unwrap(),
            gy_move: Regex::new(
                r"\b(return|put) ([^.]*?) from (your|a|an opponent's|any|their|its owner's|target player's|all|that player's) graveyards? (on)?to (the battlefield|your hand|its owner's hand|their owners?'? hands?|the top of your library)",
            )
            .unwrap(),
            gy_cast: Regex::new(r"\b(cast|play) ([^.]*?) from your graveyard").unwrap(),
            add_mana: Regex::new(r"\badd (\{[wubrgcs]\}|one mana|two mana|three mana|x mana|an amount of|mana equal|that much mana|\w+ mana)").unwrap(),
            extra_mana: Regex::new(r"\b(adds?|produces?) (an |one )?additional (\{[wubrgc]\}|mana|one mana)|\bproduces? twice as much").unwrap(),
            activated: Regex::new(r#"(?m)^[^\n:"•]{1,100}:"#).unwrap(),
            tribe,
            tribe_base,
            token_after: Regex::new(r"^( [A-Z][a-z]+)*( artifact| enchantment)?( creature)? tokens?").unwrap(),
        }
    }

    /// Texto oracle sem lembretes, mantendo maiúsculas (usado para exibir/buscar e para tribal).
    pub fn strip_reminder(&self, text: &str) -> String {
        self.reminder.replace_all(text, "").into_owned()
    }

    fn normalize(&self, names: &[&str], text: &str) -> String {
        let mut t = self.strip_reminder(text);
        let mut ns: Vec<&str> = names.iter().copied().filter(|n| !n.is_empty()).collect();
        // apelido de lendárias: "Braids, Arisen Nightmare" -> "Braids"
        for n in names {
            if let Some((short, _)) = n.split_once(", ")
                && short.len() >= 3 {
                    ns.push(short);
                }
        }
        ns.sort_by_key(|n| std::cmp::Reverse(n.len()));
        for n in ns {
            t = t.replace(n, "~");
        }
        let t = t.to_lowercase();
        let t = self.this_obj.replace_all(&t, "~");
        self.typed_card.replace_all(&t, "${1}_${2}").into_owned()
    }

    /// Tipos de criatura mencionados no texto (não conta criação de fichas).
    fn tribes(&self, text_no_reminder: &str, names: &[&str]) -> Vec<String> {
        let Some(re) = &self.tribe else { return vec![] };
        let mut t = text_no_reminder.to_string();
        for n in names {
            if !n.is_empty() {
                t = t.replace(n, "~");
            }
        }
        let mut out = BTreeSet::new();
        for m in re.find_iter(&t) {
            let after = &t[m.end()..];
            if self.token_after.is_match(after) {
                continue; // "create a 1/1 green Elf Warrior creature token"
            }
            if let Some(b) = self.tribe_base.get(m.as_str()) {
                out.insert(b.clone());
            }
        }
        if t.to_lowercase().contains("choose a creature type") {
            out.insert("escolhido".into());
        }
        out.into_iter().collect()
    }

    pub fn classify(&self, c: &Input) -> (Tags, Vec<String>) {
        let t = self.normalize(&c.names, c.text);
        let has_type = |x: &str| c.types.iter().any(|y| y == x);
        let kw: Vec<String> = c.keywords.iter().map(|k| k.to_lowercase()).collect();
        let has_kw = |k: &str| kw.iter().any(|x| x == k);
        let is_land = has_type("Land");

        let mut tags = Tags::default();
        self.function.tag(&t, &mut tags.function);
        self.trigger.tag(&t, &mut tags.trigger);
        self.target.tag(&t, &mut tags.target);
        self.theme.tag(&t, &mut tags.theme);
        let f = &mut tags.function;

        // ---- mana: quem gera mana e não é terreno é ramp
        let colors: BTreeSet<&str> =
            c.produced_mana.iter().map(String::as_str).filter(|m| "WUBRG".contains(*m)).collect();
        if colors.len() >= 2 {
            f.insert("fixing".into());
        }
        // Só conta como fonte de mana quem tem a própria habilidade "add ..." no texto
        // (sem lembretes). O Scryfall põe em produced_mana as cores de um Treasure
        // criado, o que transformava Shambling Ghast em "mana_dork".
        if !is_land && self.add_mana.is_match(&t) {
            f.insert("ramp".into());
            if has_type("Creature") {
                f.insert("mana_dork".into());
            } else if has_type("Artifact") {
                f.insert("mana_rock".into());
            } else if has_type("Instant") || has_type("Sorcery") {
                f.insert("ritual".into());
            }
        }
        if !is_land && (f.contains("treasure") || f.contains("extra_land")) {
            f.insert("ramp".into());
        }
        // dobradores de mana: "tapped for mana ... adds", "adds an additional {B}" (Crypt Ghast, High Tide)
        if (t.contains("tapped for mana") && t.contains("add")) || self.extra_mana.is_match(&t) {
            f.insert("ramp".into());
        }

        // ---- "search your library": tutor x busca de terreno
        for cap in self.search.captures_iter(&t) {
            if &cap[1] != "your" {
                continue; // ex.: Path to Exile dá o terreno ao oponente
            }
            let obj = &cap[2];
            let first = obj.split(',').next().unwrap_or(obj);
            let landish = ["land", "plains", "island", "swamp", "mountain", "forest"].iter().any(|w| first.contains(w));
            if landish {
                if is_land {
                    f.insert("fixing".into());
                } else if obj.contains("onto the battlefield") {
                    f.insert("land_ramp".into());
                    f.insert("ramp".into());
                } else {
                    f.insert("fixing".into());
                }
            } else if !obj.contains("named") {
                f.insert("tutor".into());
            }
        }

        // ---- cemitério: recursão/reanimação (a própria carta voltando não conta)
        for cap in self.gy_move.captures_iter(&t) {
            let obj = cap[2].trim();
            if obj.starts_with('~') || obj.starts_with("it ") || obj == "it" {
                tags.theme.insert("graveyard".into());
                if obj.starts_with('~') {
                    f.insert("recurring".into());
                }
                continue;
            }
            if cap[5].contains("battlefield") {
                f.insert("reanimation".into());
            } else {
                f.insert("recursion".into());
            }
        }
        if t.contains("enchant creature_card in a graveyard") {
            f.insert("reanimation".into());
        }
        for cap in self.gy_cast.captures_iter(&t) {
            if cap[2].trim_start().starts_with('~') {
                f.insert("recurring".into()); // Gravecrawler, Squee
            } else {
                f.insert("recursion".into());
            }
        }
        if t.contains("from your graveyard or from exile") && t.contains("cast ~") {
            f.insert("recurring".into());
        }

        // ---- blink não é remoção (Ephemerate, Flickerwisp)
        if f.contains("blink") && !t.contains("destroy") && !t.contains("damage") && !t.contains("sacrifices") {
            f.remove("removal");
        }

        // ---- keywords
        if has_kw("lifelink") {
            f.insert("lifegain".into());
        }
        if ["infect", "toxic", "poisonous"].iter().any(|k| has_kw(k)) {
            f.insert("poison".into());
        }
        if has_kw("proliferate") {
            f.insert("proliferate".into());
        }
        if ["populate", "amass", "incubate", "fabricate", "afterlife", "myriad", "squad", "offspring", "embalm", "eternalize", "encore"]
            .iter()
            .any(|k| has_kw(k))
        {
            f.insert("token_maker".into());
        }
        if has_kw("cascade") || has_kw("discover") {
            f.insert("impulse_draw".into());
        }
        if has_kw("dredge") {
            f.insert("self_mill".into());
            f.insert("recurring".into());
        }
        if ["undying", "persist", "escape", "disturb", "retrace"].iter().any(|k| has_kw(k)) {
            f.insert("recurring".into());
        }
        if f.contains("discard") {
            f.insert("hand_disruption".into());
        }
        if f.contains("edict") {
            f.insert("removal".into());
        }
        if has_kw("landfall") {
            tags.trigger.insert("landfall".into());
        }
        if has_kw("prowess") || has_kw("magecraft") {
            tags.trigger.insert("cast_spell".into());
            tags.theme.insert("spellslinger".into());
        }
        let activated_kw = ["equip", "crew", "reconfigure", "fortify", "outlast", "level up", "boast", "cycling", "ninjutsu", "transmute"];
        if self.activated.is_match(&t) || activated_kw.iter().any(|k| has_kw(k)) {
            tags.trigger.insert("activated".into());
        }

        // ---- temas derivados das outras tags
        let f = &tags.function;
        let tr = &tags.trigger;
        let th = &mut tags.theme;
        let mut add = |cond: bool, name: &str| {
            if cond {
                th.insert(name.to_string());
            }
        };
        add(
            f.contains("sacrifice_outlet") || tr.contains("sacrifice_trigger") || f.contains("fling")
                || (f.contains("death_value") && has_type("Creature")),
            "aristocrats",
        );
        add(f.contains("discard_outlet"), "discard");
        add(f.contains("recurring") && has_type("Creature"), "graveyard");
        add(f.contains("token_maker") || tr.contains("token_trigger") || (f.contains("doubler") && t.contains("token")), "tokens");
        add(f.contains("plus_counters") || f.contains("proliferate") || tr.contains("counter_trigger"), "counters");
        add(f.contains("reanimation"), "reanimator");
        add(
            f.contains("recursion") || f.contains("reanimation") || f.contains("self_mill")
                || ["flashback", "escape", "unearth", "delve", "dredge", "disturb", "embalm", "eternalize", "retrace", "jump-start", "aftermath", "scavenge", "encore"]
                    .iter()
                    .any(|k| has_kw(k)),
            "graveyard",
        );
        add(f.contains("extra_land") || f.contains("land_ramp") || tr.contains("landfall"), "lands");
        add(tr.contains("lifegain_trigger"), "lifegain");
        add(f.contains("mill"), "mill");
        add(f.contains("blink") || f.contains("trigger_doubler"), "blink");
        add(f.contains("tax_stax") || f.contains("land_destruction"), "stax");
        add(f.contains("ritual") || has_kw("storm"), "storm");
        add(f.contains("wheel"), "wheels");
        add(f.contains("treasure"), "treasure");
        add(c.subtypes.iter().any(|s| s == "Vehicle"), "vehicles");
        add(c.subtypes.iter().any(|s| s == "Equipment"), "voltron");
        add(f.contains("poison"), "poison");
        add(tr.contains("discard_trigger") || has_kw("madness"), "discard");

        let plain = self.strip_reminder(c.text);
        let tribes = self.tribes(&plain, &c.names);
        if !tribes.is_empty() {
            tags.theme.insert("tribal".into());
        }
        (tags, tribes)
    }
}

/// Aplica os ajustes manuais de `MANUAL` a uma carta.
pub fn apply_manual(name: &str, tags: &mut Tags) {
    let Some((_, add, remove)) = MANUAL.iter().find(|(n, _, _)| *n == name) else { return };
    fn slot<'t>(tags: &'t mut Tags, t: &str) -> Option<(String, &'t mut BTreeSet<String>)> {
        let (cat, tag) = match t.split_once(':') {
            Some((c, x)) => (c.to_string(), x.to_string()),
            None => (category_of(t)?.to_string(), t.to_string()),
        };
        let set = match cat.as_str() {
            "function" => &mut tags.function,
            "trigger" => &mut tags.trigger,
            "target" => &mut tags.target,
            "theme" => &mut tags.theme,
            _ => return None,
        };
        Some((tag, set))
    }
    for t in *add {
        if let Some((tag, set)) = slot(tags, t) {
            set.insert(tag);
        }
    }
    for t in *remove {
        if let Some((tag, set)) = slot(tags, t) {
            set.remove(&tag);
        }
    }
}

// ---------------------------------------------------------------- testes

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(name: &str, type_line: &str, text: &str, keywords: &[&str], mana: &[&str]) -> Tags {
        let types: Vec<String> = type_line.split(" — ").next().unwrap().split_whitespace().map(String::from).collect();
        let subtypes: Vec<String> = type_line.split(" — ").nth(1).unwrap_or("").split_whitespace().map(String::from).collect();
        let kw: Vec<String> = keywords.iter().map(|s| s.to_string()).collect();
        let pm: Vec<String> = mana.iter().map(|s| s.to_string()).collect();
        let ct: BTreeSet<String> = ["Elf", "Goblin", "Zombie", "Soldier", "Warrior"].iter().map(|s| s.to_string()).collect();
        let c = Classifier::new(&ct);
        let input = Input { names: vec![name], text, types: &types, subtypes: &subtypes, keywords: &kw, produced_mana: &pm };
        let (mut tags, _) = c.classify(&input);
        apply_manual(name, &mut tags);
        tags
    }

    #[test]
    fn blood_artist_dies_trigger() {
        let t = classify("Blood Artist", "Creature — Vampire",
            "Whenever this creature or another creature dies, target player loses 1 life and you gain 1 life.", &[], &[]);
        assert!(t.trigger.contains("dies"));
        assert!(t.function.contains("drain"));
        assert!(t.theme.contains("aristocrats"));
    }

    #[test]
    fn sun_titan_etb_and_recursion() {
        let t = classify("Sun Titan", "Creature — Giant",
            "Vigilance\nWhenever this creature enters or attacks, you may return target permanent card with mana value 3 or less from your graveyard to the battlefield.",
            &["Vigilance"], &[]);
        assert!(t.trigger.contains("etb"));
        assert!(t.trigger.contains("attacks"));
        assert!(t.function.contains("reanimation"));
        assert!(!t.function.contains("removal"));
    }

    #[test]
    fn sacrifice_outlets_are_activated() {
        let t = classify("Ashnod's Altar", "Artifact", "Sacrifice a creature: Add {C}{C}.", &[], &["C"]);
        assert!(t.trigger.contains("activated"));
        assert!(t.function.contains("sacrifice_outlet"));
        assert!(t.function.contains("mana_rock"));
        let t = classify("Viscera Seer", "Creature — Vampire Wizard", "Sacrifice a creature: Scry 1.", &[], &[]);
        assert!(t.trigger.contains("activated"));
        assert!(t.function.contains("sacrifice_outlet"));
    }

    #[test]
    fn haste_only_when_granted() {
        let t = classify("Craterhoof Behemoth", "Creature — Beast",
            "Haste\nWhen this creature enters, creatures you control gain trample and get +X/+X until end of turn, where X is the number of creatures you control.",
            &["Haste"], &[]);
        assert!(!t.function.contains("haste_enabler"));
        assert!(t.function.contains("anthem"));
        assert!(t.function.contains("pump"));
        assert!(t.function.contains("evasion"));
        let t = classify("Swiftfoot Boots", "Artifact — Equipment",
            "Equipped creature has hexproof and haste.\nEquip {1}", &["Equip"], &[]);
        assert!(t.function.contains("haste_enabler"));
        assert!(t.function.contains("protection"));
    }

    #[test]
    fn removal_vs_blink_and_graveyard() {
        let t = classify("Swords to Plowshares", "Instant", "Exile target creature. Its controller gains life equal to its power.", &[], &[]);
        assert!(t.function.contains("removal"));
        assert!(!t.function.contains("lifegain"));
        let t = classify("Ephemerate", "Instant",
            "Exile target creature you control, then return it to the battlefield under its owner's control.\nRebound", &["Rebound"], &[]);
        assert!(t.function.contains("blink"));
        assert!(!t.function.contains("removal"));
        let t = classify("Bojuka Bog", "Land", "This land enters tapped.\nWhen this land enters, exile target player's graveyard.\n{T}: Add {B}.", &[], &["B"]);
        assert!(t.function.contains("graveyard_hate"));
        assert!(!t.function.contains("removal"));
        assert!(!t.function.contains("ramp"));
    }

    #[test]
    fn counterspell_is_not_tax() {
        let t = classify("Mana Leak", "Instant", "Counter target spell unless its controller pays {3}.", &[], &[]);
        assert!(t.function.contains("counterspell"));
        assert!(!t.function.contains("tax_stax"));
    }

    #[test]
    fn tutor_vs_land_search() {
        let t = classify("Demonic Tutor", "Sorcery", "Search your library for a card, put that card into your hand, then shuffle.", &[], &[]);
        assert!(t.function.contains("tutor"));
        assert!(!t.function.contains("card_draw"));
        let t = classify("Cultivate", "Sorcery",
            "Search your library for up to two basic land cards, reveal those cards, put one onto the battlefield tapped and the other into your hand, then shuffle.", &[], &[]);
        assert!(t.function.contains("land_ramp"));
        assert!(t.function.contains("ramp"));
        assert!(!t.function.contains("tutor"));
        let t = classify("Path to Exile", "Instant",
            "Exile target creature. Its controller may search their library for a basic land card, put that card onto the battlefield tapped, then shuffle.", &[], &[]);
        assert!(t.function.contains("removal"));
        assert!(!t.function.contains("ramp"));
    }

    #[test]
    fn mana_creatures_and_rituals() {
        let t = classify("Llanowar Elves", "Creature — Elf Druid", "{T}: Add {G}.", &[], &["G"]);
        assert!(t.function.contains("mana_dork"));
        let t = classify("Dark Ritual", "Instant", "Add {B}{B}{B}.", &[], &["B"]);
        assert!(t.function.contains("ritual"));
        assert!(t.theme.contains("storm"));
        let t = classify("Command Tower", "Land", "{T}: Add one mana of any color in your commander's color identity.", &[], &["W", "U", "B", "R", "G"]);
        assert!(!t.function.contains("ramp"));
        assert!(t.function.contains("fixing"));
    }

    #[test]
    fn wrath_and_reanimate() {
        let t = classify("Wrath of God", "Sorcery", "Destroy all creatures. They can't be regenerated.", &[], &[]);
        assert!(t.function.contains("board_wipe"));
        let t = classify("Reanimate", "Sorcery",
            "Put target creature card from a graveyard onto the battlefield under your control. You lose life equal to its mana value.", &[], &[]);
        assert!(t.function.contains("reanimation"));
        assert!(t.target.contains("graveyard_card"));
        assert!(!t.target.contains("creature"));
        assert!(t.theme.contains("reanimator"));
    }

    #[test]
    fn tribal_ignores_token_creation() {
        let ct: BTreeSet<String> = ["Elf", "Soldier"].iter().map(|s| s.to_string()).collect();
        let c = Classifier::new(&ct);
        let types = vec!["Creature".to_string()];
        let none: Vec<String> = vec![];
        let lord = Input { names: vec!["Elvish Archdruid"], text: "Other Elf creatures you control get +1/+1.\n{T}: Add {G} for each Elf you control.", types: &types, subtypes: &none, keywords: &none, produced_mana: &none };
        assert_eq!(c.classify(&lord).1, vec!["Elf".to_string()]);
        let maker = Input { names: vec!["Raise the Alarm"], text: "Create two 1/1 white Soldier creature tokens.", types: &types, subtypes: &none, keywords: &none, produced_mana: &none };
        assert!(c.classify(&maker).1.is_empty());
    }

    #[test]
    fn opponent_drawing_is_not_card_draw() {
        let t = classify("Smothering Tithe", "Enchantment",
            "Whenever an opponent draws a card, that player may pay {2}. If they don't, you create a Treasure token.", &[], &[]);
        assert!(!t.function.contains("card_draw"));
        assert!(t.function.contains("treasure"));
        assert!(t.theme.contains("wheels"));
    }

    #[test]
    fn divided_damage_is_removal() {
        let t = classify("Fire // Ice", "Instant // Instant",
            "Fire deals 2 damage divided as you choose among one or two targets.\nTap target permanent.\nDraw a card.", &[], &[]);
        assert!(t.function.contains("removal"));
        assert!(t.function.contains("burn"));
    }

    // ---- revisão 2: correções (A) e tags novas (B)

    #[test]
    fn treasure_maker_is_not_mana_dork() {
        let t = classify("Shambling Ghast", "Creature — Zombie",
            "When this creature dies, choose one —\n• Target creature an opponent controls gets -1/-1 until end of turn.\n• Create a Treasure token. (It's an artifact with \"{T}, Sacrifice this artifact: Add one mana of any color.\")",
            &[], &["W", "U", "B", "R", "G"]);
        assert!(!t.function.contains("mana_dork"));
        assert!(t.function.contains("treasure"));
        assert!(t.function.contains("death_value"));
        assert!(t.theme.contains("aristocrats"));
    }

    #[test]
    fn single_target_pump_is_not_anthem() {
        let t = classify("Nezumi Linkbreaker", "Creature — Rat Warlock",
            "When this creature dies, create a 1/1 red Mercenary creature token with \"{T}: Target creature you control gets +1/+0 until end of turn. Activate only as a sorcery.\"", &[], &[]);
        assert!(!t.function.contains("anthem"));
        let t = classify("Glorious Anthem", "Enchantment", "Creatures you control get +1/+1.", &[], &[]);
        assert!(t.function.contains("anthem"));
        let t = classify("Collective Blessing", "Enchantment", "Each creature you control gets +3/+3.", &[], &[]);
        assert!(t.function.contains("anthem"));
    }

    #[test]
    fn damage_equal_to_power_is_burn_and_fling() {
        let t = classify("Fling", "Instant",
            "As an additional cost to cast this spell, sacrifice a creature.\nFling deals damage equal to the sacrificed creature's power to any target.", &[], &[]);
        assert!(t.function.contains("burn"));
        assert!(t.function.contains("removal"));
        assert!(t.function.contains("fling"));
        let t = classify("Jarad, Golgari Lich Lord", "Legendary Creature — Zombie Elf",
            "Jarad gets +1/+1 for each creature card in your graveyard.\n{1}{B}{G}, Sacrifice another creature: Each opponent loses life equal to the sacrificed creature's power.\nSacrifice a Swamp and a Forest: Return this card from your graveyard to your hand.", &[], &[]);
        assert!(t.function.contains("fling"));
        assert!(t.function.contains("scaling"));
        assert!(t.function.contains("recurring"));
        let t = classify("Altar of Dementia", "Artifact", "Sacrifice a creature: Target player mills cards equal to the sacrificed creature's power.", &[], &[]);
        assert!(!t.function.contains("fling"));
    }

    #[test]
    fn monarch_is_card_draw() {
        let t = classify("Thorn of the Black Rose", "Creature — Human Assassin",
            "Deathtouch\nWhen this creature enters, you become the monarch.", &["Deathtouch"], &[]);
        assert!(t.function.contains("card_draw"));
    }

    #[test]
    fn dredge_and_undying_are_recurring() {
        let t = classify("Stinkweed Imp", "Creature — Imp",
            "Flying\nWhenever this creature deals combat damage to a creature, destroy that creature.\nDredge 5", &["Flying", "Dredge"], &[]);
        assert!(t.function.contains("self_mill"));
        assert!(t.function.contains("recurring"));
        let t = classify("Young Wolf", "Creature — Wolf", "Undying", &["Undying"], &[]);
        assert!(t.function.contains("recurring"));
        let t = classify("Gravecrawler", "Creature — Zombie",
            "This creature can't block.\nYou may cast this card from your graveyard as long as you control a Zombie.", &[], &[]);
        assert!(t.function.contains("recurring"));
        assert!(!t.function.contains("recursion"));
    }

    #[test]
    fn death_value_is_fodder_not_payoff() {
        let t = classify("Nested Shambler", "Creature — Zombie",
            "When this creature dies, create X tapped 1/1 green Squirrel creature tokens, where X is this creature's power.", &[], &[]);
        assert!(t.function.contains("death_value"));
        let t = classify("Blood Artist", "Creature — Vampire",
            "Whenever this creature or another creature dies, target player loses 1 life and you gain 1 life.", &[], &[]);
        assert!(!t.function.contains("death_value"));
    }

    #[test]
    fn discard_outlet_edict_and_hand_disruption() {
        let t = classify("Tortured Existence", "Enchantment",
            "{B}, Discard a creature card: Return target creature card from your graveyard to your hand.", &[], &[]);
        assert!(t.function.contains("discard_outlet"));
        assert!(t.theme.contains("discard"));
        let t = classify("Accursed Marauder", "Creature — Zombie Warrior",
            "When this creature enters, each player sacrifices a nontoken creature of their choice.", &[], &[]);
        assert!(t.function.contains("edict"));
        assert!(t.function.contains("removal"));
        let t = classify("Village Rites", "Instant",
            "As an additional cost to cast this spell, sacrifice a creature.\nDraw two cards.", &[], &[]);
        assert!(!t.function.contains("edict"));
        let t = classify("Chittering Rats", "Creature — Rat",
            "When this creature enters, target opponent puts a card from their hand on top of their library.", &[], &[]);
        assert!(t.function.contains("hand_disruption"));
        let t = classify("Duress", "Sorcery",
            "Target opponent reveals their hand. You choose a noncreature, nonland card from it. That player discards that card.", &[], &[]);
        assert!(t.function.contains("hand_disruption"));
    }

    #[test]
    fn free_spell_and_scaling() {
        let t = classify("Snuff Out", "Instant",
            "If you control a Swamp, you may pay 4 life rather than pay this spell's mana cost.\nDestroy target nonblack creature. It can't be regenerated.", &[], &[]);
        assert!(t.function.contains("free_spell"));
        assert!(t.function.contains("removal"));
        let t = classify("Lord of Extinction", "Creature — Elemental",
            "Lord of Extinction's power and toughness are each equal to the number of cards in all graveyards.", &[], &[]);
        assert!(t.function.contains("scaling"));
        assert!(t.theme.contains("graveyard"));
    }

    /// Regressão: tests/golden_cards.json tem cartas reais (texto oracle do Scryfall)
    /// com as tags que cada uma PRECISA ter e as que NÃO pode ter.
    /// Ao mudar uma regra, este teste mostra todas as cartas que quebraram de uma vez.
    #[test]
    fn golden_cards_regression() {
        #[derive(serde::Deserialize)]
        struct Golden {
            name: String,
            type_line: String,
            text: String,
            keywords: Vec<String>,
            produced_mana: Vec<String>,
            must: Vec<String>,
            must_not: Vec<String>,
        }
        let golden: Vec<Golden> = serde_json::from_str(include_str!("../tests/golden_cards.json")).unwrap();
        let ct: BTreeSet<String> = ["Elf", "Goblin", "Zombie", "Vampire", "Human"].iter().map(|s| s.to_string()).collect();
        let c = Classifier::new(&ct);
        let mut errors = vec![];
        for g in &golden {
            let (main, sub) = g.type_line.split_once(" — ").unwrap_or((&g.type_line, ""));
            let types: Vec<String> = main.split_whitespace().map(String::from).collect();
            let subtypes: Vec<String> = sub.split_whitespace().map(String::from).collect();
            let input = Input {
                names: vec![g.name.as_str()],
                text: &g.text,
                types: &types,
                subtypes: &subtypes,
                keywords: &g.keywords,
                produced_mana: &g.produced_mana,
            };
            let (mut tags, _) = c.classify(&input);
            apply_manual(&g.name, &mut tags);
            for t in &g.must {
                if !tags.contains(t) {
                    errors.push(format!("{}: faltou {t}", g.name));
                }
            }
            for t in &g.must_not {
                if tags.contains(t) {
                    errors.push(format!("{}: não devia ter {t}", g.name));
                }
            }
        }
        assert!(errors.is_empty(), "{} erro(s) em {} cartas:\n{}", errors.len(), golden.len(), errors.join("\n"));
    }

    #[test]
    fn every_rule_tag_is_in_taxonomy() {
        for rules in [FUNCTION_RULES, TRIGGER_RULES, TARGET_RULES, THEME_RULES] {
            for (tag, _, _) in rules {
                assert!(pt_label(tag).is_some(), "tag sem rótulo: {tag}");
            }
        }
    }
}
