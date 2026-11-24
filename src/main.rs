//! Magic RAG — busca local de cartas de Magic (todas as ~35 mil do Scryfall).
//!
//! Comandos:
//!   magic_rag install                   fetch + build (também roda no duplo clique sem dados)
//!   magic_rag fetch                    baixa o bulk "Oracle Cards" do Scryfall para data/
//!   magic_rag build                     gera data/cards.json (tags, texto de busca)
//!   magic_rag search "<texto>" [filtros]
//!   magic_rag card "<nome>"             ficha completa de uma carta
//!   magic_rag deck "<comandante>"       sugestões por categoria para um deck de Commander
//!   magic_rag tags                      lista a taxonomia de tags e quantas cartas têm cada uma
//!   magic_rag stats [--json]            métricas da classificação (cobertura, tags por carta)
//!   magic_rag mcp                       servidor MCP (stdio) para um LLM usar como ferramenta

mod build;
mod card;
mod deck;
mod fetch;
mod mcp;
mod search;
mod tags;

use card::Card;
use search::{Hit, Index, Query, Sort};
use serde_json::{Value, json};
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::{env, process};

const RAW_FILE: &str = "scryfall-oracle-cards.jsonl.gz";
const CARDS_FILE: &str = "cards.json";

const USAGE: &str = r#"uso:
  magic_rag install                     baixa e prepara todas as cartas (fetch + build)
  magic_rag fetch                       baixa todas as cartas do Scryfall (~25 MB comprimido)
  magic_rag build                       gera data/cards.json
  magic_rag search "<texto>" [filtros]  busca cartas
  magic_rag card "<nome>" [--json]      ficha completa
  magic_rag deck "<comandante>" [--per N] [--budget USD] [--no-game-changers] [--tag T]... [--json]
  magic_rag tags                        lista as tags por categoria
  magic_rag stats [--json]              métricas da classificação (cobertura, tags por carta)
  magic_rag mcp                         servidor MCP (stdio) para um LLM usar como ferramenta

filtros do search:
  --colors W,U       identidade de cor permitida (C = só incolor)
  --has-colors W,U   a identidade precisa conter essas cores
  --cmc-min N  --cmc-max N
  --type T           tipo (Creature, Instant...; repetir = "ou": --type Instant --type Sorcery)
  --subtype S        subtipo exato (Elf, Equipment, Aura...)
  --tribe S          menciona ou é do tipo de criatura S
  --rarity R         common, uncommon, rare, mythic
  --name TXT         nome contém TXT
  --tag T            exige a tag (pode repetir; aceita "theme:tokens")
  --any-tag T        exige ao menos uma destas (pode repetir)
  --not-tag T        exclui a tag (pode repetir)
  --boost T          tag que só aumenta a nota (pode repetir)
  --format F         legal no formato (commander, modern, pauper...)
  --commander        só cartas que podem ser comandante
  --instant-speed    só instantâneos e cartas com Flash
  --price-max USD    preço máximo em dólar
  --paper            esconde cartas só digitais e cartas "Un-"
  --no-game-changers esconde Game Changers (brackets de Commander)
  --sort S           relevance|popularity|cmc|price|name|newest
  --top N            quantos resultados (padrão 10)
  --json             saída em JSON (para mandar ao LLM)

dados: pasta data/ (ou a variável MAGIC_RAG_DATA)"#;

/// Pasta de dados: $MAGIC_RAG_DATA, ./data, ou data/ ao lado do projeto do executável.
fn data_dir() -> PathBuf {
    if let Ok(d) = env::var("MAGIC_RAG_DATA") {
        return PathBuf::from(d);
    }
    let cwd = PathBuf::from("data");
    if cwd.is_dir() {
        return cwd;
    }
    if let Ok(exe) = env::current_exe() {
        for dir in exe.ancestors().skip(1).take(4) {
            let d = dir.join("data");
            if d.join(CARDS_FILE).is_file() {
                return d;
            }
        }
    }
    cwd
}

fn data_file(name: &str) -> String {
    data_dir().join(name).to_string_lossy().into_owned()
}

fn load_cards() -> Result<Vec<Card>, Box<dyn Error>> {
    let path = data_file(CARDS_FILE);
    let f = File::open(&path).map_err(|e| format!("{path}: {e} (rode antes: magic_rag install)"))?;
    Ok(serde_json::from_reader(BufReader::new(f))?)
}

fn main() {
    // `magic_rag search ... | head` fecha a saída antes do fim: sair sem pânico
    std::panic::set_hook(Box::new(|info| {
        if info.to_string().contains("failed printing to stdout") {
            process::exit(0);
        }
        eprintln!("{info}");
    }));
    let args: Vec<String> = env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("install") => install(),
        // duplo clique no .exe sem nada instalado: instala e espera o Enter
        None if !PathBuf::from(data_file(CARDS_FILE)).is_file() => {
            let r = install();
            if let Err(e) = &r {
                eprintln!("erro: {e}");
            }
            println!("\nPressione Enter para sair.");
            let _ = std::io::stdin().read_line(&mut String::new());
            process::exit(if r.is_ok() { 0 } else { 1 });
        }
        Some("fetch") => fetch::run(&rest.first().cloned().unwrap_or_else(|| data_file(RAW_FILE))),
        Some("build") => build::run(
            &rest.first().cloned().unwrap_or_else(|| data_file(RAW_FILE)),
            &rest.get(1).cloned().unwrap_or_else(|| data_file(CARDS_FILE)),
        ),
        Some("search") => run_search(rest),
        Some("card") => run_card(rest),
        Some("deck") => run_deck(rest),
        Some("tags") => run_tags(),
        Some("stats") => run_stats(rest),
        Some("mcp") => mcp::run(),
        _ => {
            eprintln!("{USAGE}");
            process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("erro: {e}");
        process::exit(1);
    }
}

fn csv_upper(s: &str) -> Vec<String> {
    s.to_uppercase().split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty() && x != "C").collect()
}

/// Baixa todas as cartas do Scryfall e gera o dataset local (fetch + build).
fn install() -> Result<(), Box<dyn Error>> {
    let raw = data_file(RAW_FILE);
    let cards = data_file(CARDS_FILE);
    println!("== Instalando as cartas do Magic em {}", data_dir().display());
    fetch::run(&raw)?;
    build::run(&raw, &cards)?;
    println!("\nPronto! Experimente:\n  magic_rag search \"\" --tag removal --format commander\n  magic_rag card \"Sol Ring\"");
    Ok(())
}

fn run_search(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut q = Query { top: 10, ..Default::default() };
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("faltou valor para {a}"));
        match a.as_str() {
            "--colors" | "--identity" => q.colors = Some(csv_upper(&val()?)),
            "--has-colors" => q.colors_include = csv_upper(&val()?),
            "--cmc-min" => q.cmc_min = Some(val()?.parse()?),
            "--cmc-max" => q.cmc_max = Some(val()?.parse()?),
            "--type" => q.types.push(val()?),
            "--subtype" => q.subtype = Some(val()?),
            "--tribe" => q.tribe = Some(val()?),
            "--rarity" => q.rarity = Some(val()?),
            "--name" => q.name = Some(val()?),
            "--tag" => q.tags.push(val()?),
            "--any-tag" => q.any_tags.push(val()?),
            "--not-tag" => q.not_tags.push(val()?),
            "--boost" => q.boost.push(val()?),
            "--format" => q.format = Some(val()?),
            "--commander" => q.commander_only = true,
            "--instant-speed" => q.instant_speed = true,
            "--price-max" => q.price_max = Some(val()?.parse()?),
            "--paper" => q.paper = true,
            "--no-game-changers" => q.no_game_changers = true,
            "--sort" => q.sort = Sort::parse(&val()?)?,
            "--top" => q.top = val()?.parse()?,
            "--json" => json = true,
            s if s.starts_with("--") => return Err(format!("opção desconhecida: {s}\n{USAGE}").into()),
            s => q.text = s.to_string(),
        }
    }
    validate_tags(q.tags.iter().chain(&q.any_tags).chain(&q.not_tags).chain(&q.boost))?;

    let cards = load_cards()?;
    let index = Index::new(&cards);
    let res = search::search(&cards, &index, &q);

    if json {
        let out: Vec<Value> = res.iter().map(|h| card_json(h.card, Some(h.score), false)).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if res.is_empty() {
        println!("Nenhuma carta encontrada.");
    }
    for h in &res {
        print_hit(h, q.has_text());
    }
    Ok(())
}

fn run_card(args: &[String]) -> Result<(), Box<dyn Error>> {
    let json = args.iter().any(|a| a == "--json");
    let name = args.iter().find(|a| !a.starts_with("--")).ok_or("informe o nome da carta")?;
    let cards = load_cards()?;
    let found = search::find_by_name(&cards, name);
    if found.is_empty() {
        println!("Nenhuma carta com esse nome.");
        return Ok(());
    }
    let shown = &found[..found.len().min(5)];
    if json {
        let out: Vec<Value> = shown.iter().map(|c| card_json(c, None, true)).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    for c in shown {
        print_full(c);
    }
    if found.len() > shown.len() {
        println!("(+{} outras cartas com \"{name}\" no nome)", found.len() - shown.len());
    }
    Ok(())
}

fn run_deck(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut o = deck::DeckOpts { per: 8, budget: None, no_game_changers: false, extra_tags: vec![] };
    let mut json = false;
    let mut name = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("faltou valor para {a}"));
        match a.as_str() {
            "--per" => o.per = val()?.parse()?,
            "--budget" => o.budget = Some(val()?.parse()?),
            "--no-game-changers" => o.no_game_changers = true,
            "--tag" => o.extra_tags.push(val()?),
            "--json" => json = true,
            s if s.starts_with("--") => return Err(format!("opção desconhecida: {s}\n{USAGE}").into()),
            s => name = Some(s.to_string()),
        }
    }
    validate_tags(o.extra_tags.iter())?;
    let name = name.ok_or("informe o nome do comandante")?;
    let cards = load_cards()?;
    let index = Index::new(&cards);
    let plan = deck::build(&cards, &index, &name, &o)?;

    if json {
        let out = json!({
            "commander": card_json(plan.commander, None, true),
            "sections": plan.sections.iter().map(|s| json!({
                "key": s.key, "title": s.title,
                "cards": s.cards.iter().map(|(sc, c)| card_json(c, Some(*sc), false)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let c = plan.commander;
    println!("Comandante: {}  {}  — {}", c.name, c.mana_cost, c.type_line);
    println!("Identidade: {}   Temas: {}", fmt_colors(&c.color_identity), join(&c.tags.theme));
    println!();
    for s in &plan.sections {
        println!("== {} [{}]", s.title, s.key);
        if s.cards.is_empty() {
            println!("   (nada encontrado)");
        }
        for (sc, c) in &s.cards {
            println!("   [{sc:.2}] {}  {}  — {}{}", c.name, c.mana_cost, c.type_line, fmt_price(c));
        }
        println!();
    }
    Ok(())
}

fn run_tags() -> Result<(), Box<dyn Error>> {
    let cards = load_cards().ok();
    for (cat, list) in tags::TAXONOMY {
        println!("== {cat}");
        for (tag, label) in *list {
            let n = cards.as_ref().map(|cs| {
                cs.iter().filter(|c| c.tags.category(cat).is_some_and(|s| s.contains(*tag))).count()
            });
            match n {
                Some(n) => println!("  {tag:<18} {n:>6}  {label}"),
                None => println!("  {tag:<18} {label}"),
            }
        }
        println!();
    }
    Ok(())
}

/// Métricas da classificação, para medir se uma mudança de regra melhorou ou piorou.
/// `--json` gera saída para salvar e comparar entre versões.
fn run_stats(args: &[String]) -> Result<(), Box<dyn Error>> {
    let json_out = args.iter().any(|a| a == "--json");
    let cards = load_cards()?;
    // só cartas "de verdade": sem terrenos, sem Un- e sem só-digitais
    let pool: Vec<&Card> = cards
        .iter()
        .filter(|c| !c.types.iter().any(|t| t == "Land") && !c.funny && !c.digital_only)
        .collect();

    let band = |lim: Option<u32>| {
        let s: Vec<&&Card> = pool
            .iter()
            .filter(|c| lim.is_none_or(|l| c.edhrec_rank.is_some_and(|r| r <= l)))
            .collect();
        let no_func = s.iter().filter(|c| c.tags.function.is_empty()).count();
        let no_tag = s.iter().filter(|c| c.tags.all().next().is_none()).count();
        (s.len(), no_func, no_tag)
    };
    let bands = [("top 1000", Some(1000)), ("top 5000", Some(5000)), ("todas", None)];
    let coverage: Vec<(String, usize, usize, usize)> =
        bands.iter().map(|(n, l)| { let (a, b, c) = band(*l); (n.to_string(), a, b, c) }).collect();

    let mut per_tag = vec![];
    for (cat, list) in tags::TAXONOMY {
        for (tag, _) in *list {
            let n = cards.iter().filter(|c| c.tags.category(cat).is_some_and(|s| s.contains(*tag))).count();
            per_tag.push((format!("{cat}:{tag}"), n));
        }
    }
    let avg = pool.iter().map(|c| c.tags.function.len()).sum::<usize>() as f64 / pool.len().max(1) as f64;

    if json_out {
        let out = json!({
            "cards": cards.len(),
            "nonland_pool": pool.len(),
            "avg_function_tags": (avg * 100.0).round() / 100.0,
            "coverage": coverage.iter().map(|(n, t, f, a)| json!({"band": n, "cards": t, "no_function": f, "no_tag": a})).collect::<Vec<_>>(),
            "per_tag": per_tag.iter().map(|(t, n)| (t.clone(), json!(n))).collect::<serde_json::Map<_, _>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("{} cartas no dataset; {} não-terrenos reais (sem Un- e só-digitais)", cards.len(), pool.len());
    println!("média de tags de função por carta: {avg:.2}\n");
    println!("cobertura (não-terrenos):");
    for (n, t, f, a) in &coverage {
        println!("  {n:<9} {t:>6} cartas | sem função: {f:>5} ({:>4.1}%) | sem nenhuma tag: {a:>5} ({:>4.1}%)",
            100.0 * *f as f64 / *t as f64, 100.0 * *a as f64 / *t as f64);
    }
    let dead: Vec<&String> = per_tag.iter().filter(|(_, n)| *n == 0).map(|(t, _)| t).collect();
    println!("\ntags sem nenhuma carta (regra morta?): {}", if dead.is_empty() { "nenhuma".into() } else { join(dead) });
    let mut sorted = per_tag.clone();
    sorted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("\nmaiores: {}", sorted.iter().take(10).map(|(t, n)| format!("{t}={n}")).collect::<Vec<_>>().join(", "));
    println!("menores: {}", sorted.iter().rev().filter(|(_, n)| *n > 0).take(10).map(|(t, n)| format!("{t}={n}")).collect::<Vec<_>>().join(", "));
    Ok(())
}

fn validate_tags<'a>(tags: impl Iterator<Item = &'a String>) -> Result<(), String> {
    for t in tags {
        let t = t.to_lowercase();
        let (cat, name) = t.split_once(':').map_or((None, t.as_str()), |(c, n)| (Some(c), n));
        let ok = tags::TAXONOMY.iter().any(|(c, list)| {
            cat.is_none_or(|x| x == *c) && list.iter().any(|(n, _)| *n == name)
        });
        if !ok {
            let similar = similar_tags(name);
            let hint = if similar.is_empty() { String::new() } else { format!(" Você quis dizer: {}?", similar.join(", ")) };
            return Err(format!("tag desconhecida: {t}.{hint} (lista completa: list_tags / magic_rag tags)"));
        }
    }
    Ok(())
}

/// Tags parecidas com um nome errado: "haste" -> haste_enabler, "removel" -> removal.
fn similar_tags(name: &str) -> Vec<String> {
    let all = tags::TAXONOMY.iter().flat_map(|(c, l)| l.iter().map(move |(n, _)| (*c, *n)));
    let mut scored: Vec<(usize, String)> = all
        .filter_map(|(cat, tag)| {
            let shared = tag.chars().zip(name.chars()).take_while(|(a, b)| a == b).count();
            let contains = tag.contains(name) || name.contains(tag);
            let parts = tag.split('_').any(|p| p.len() >= 3 && name.contains(p));
            let score = if contains { 100 } else { 0 } + if parts { 50 } else { 0 } + if shared >= 4 { shared } else { 0 };
            (score > 0).then(|| (score, format!("{cat}:{tag}")))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(3).map(|(_, t)| t).collect()
}

// ---------------------------------------------------------------- saída

fn join<'a>(it: impl IntoIterator<Item = &'a String>) -> String {
    it.into_iter().map(String::as_str).collect::<Vec<_>>().join(", ")
}

fn fmt_colors(ci: &[String]) -> String {
    if ci.is_empty() { "incolor".into() } else { ci.join("") }
}

fn fmt_price(c: &Card) -> String {
    c.price_usd.map_or(String::new(), |p| format!("  US${p:.2}"))
}

fn print_hit(h: &Hit, has_text: bool) {
    let c = h.card;
    let head = if has_text {
        format!("[{:5.2}]", h.score)
    } else {
        c.edhrec_rank.map_or("[  -  ]".into(), |r| format!("[#{r}]"))
    };
    println!("{head} {}  {}  — {}{}", c.name, c.mana_cost, c.type_line, fmt_price(c));
    let tags: Vec<&String> = c.tags.function.iter().chain(&c.tags.trigger).collect();
    println!("        tags: {}", tags.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    if !c.tags.theme.is_empty() {
        println!("        temas: {}", join(&c.tags.theme));
    }
    println!("        {}\n", c.oracle_text.replace('\n', "\n        "));
}

fn print_full(c: &Card) {
    println!("{}  {}", c.name, c.mana_cost);
    println!("{}", c.type_line);
    println!("{}", c.oracle_text);
    if let (Some(p), Some(t)) = (&c.power, &c.toughness) {
        println!("{p}/{t}");
    }
    if let Some(l) = &c.loyalty {
        println!("Lealdade: {l}");
    }
    println!("---");
    println!("Identidade: {}  cmc: {}  raridade: {}  coleção: {} ({})", fmt_colors(&c.color_identity), c.cmc, c.rarity, c.set_name, c.released_at);
    println!("EDHREC: {}  preço: {}  comandante: {}  game changer: {}",
        c.edhrec_rank.map_or("-".into(), |r| format!("#{r}")),
        c.price_usd.map_or("-".into(), |p| format!("US${p:.2}")),
        if c.can_be_commander { "sim" } else { "não" },
        if c.game_changer { "sim" } else { "não" });
    println!("função: {}", join(&c.tags.function));
    println!("gatilho: {}", join(&c.tags.trigger));
    println!("alvo: {}", join(&c.tags.target));
    println!("tema: {}", join(&c.tags.theme));
    if !c.tribes.is_empty() {
        println!("tribos: {}", join(&c.tribes));
    }
    println!("formatos: {}", join(&c.legal_formats));
    if let Some(u) = &c.scryfall_uri {
        println!("{u}");
    }
    println!();
}

fn card_json(c: &Card, score: Option<f64>, full: bool) -> Value {
    let mut v = json!({
        "name": c.name, "mana_cost": c.mana_cost, "cmc": c.cmc, "type_line": c.type_line,
        "oracle_text": c.oracle_text, "color_identity": c.color_identity,
        "power": c.power, "toughness": c.toughness, "loyalty": c.loyalty,
        "rarity": c.rarity, "edhrec_rank": c.edhrec_rank, "price_usd": c.price_usd,
        "game_changer": c.game_changer, "can_be_commander": c.can_be_commander,
        "tags": c.tags, "tribes": c.tribes,
    });
    if let Some(s) = score {
        v["score"] = json!((s * 100.0).round() / 100.0);
    }
    if full {
        v["keywords"] = json!(c.keywords);
        v["legal_formats"] = json!(c.legal_formats);
        v["set_name"] = json!(c.set_name);
        v["released_at"] = json!(c.released_at);
        v["scryfall_uri"] = json!(c.scryfall_uri);
        v["image"] = json!(c.image);
    }
    v
}
