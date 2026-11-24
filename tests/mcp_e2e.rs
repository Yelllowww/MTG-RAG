//! Teste de ponta a ponta do servidor MCP: sobe o executável real (`magic_rag mcp`)
//! e conversa com ele por JSON-RPC, como um cliente (Claude Code, Claude Desktop...).
//!
//! Precisa das cartas instaladas (`magic_rag install`). Sem `data/cards.json`, os
//! testes são pulados com um aviso, para não quebrar quem só clonou o repositório.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start() -> Option<Server> {
        if !Path::new("data/cards.json").is_file() {
            eprintln!("AVISO: data/cards.json não existe; rode `magic_rag install`. Teste MCP pulado.");
            return None;
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_magic_rag"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("não subiu o servidor");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut s = Server { child, stdin, stdout, next_id: 0 };
        let init = s.rpc("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "mtg-rag");
        s.send_raw(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string());
        Some(s)
    }

    fn send_raw(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("resposta inválida ({e}): {line}"))
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.send_raw(&json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}).to_string());
        self.read()
    }

    /// Chama uma ferramenta. Ok(json) no sucesso, Err(texto) se isError.
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        let text = res["content"][0]["text"].as_str().unwrap().to_string();
        if res["isError"].as_bool().unwrap_or(false) { Err(text) } else { Ok(serde_json::from_str(&text).unwrap()) }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn names(v: &Value) -> Vec<String> {
    v["cards"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect()
}

fn problems(v: &Value) -> String {
    v["problems"].to_string()
}

#[test]
fn mcp_end_to_end() {
    let Some(mut s) = Server::start() else { return };

    // ---- protocolo
    let tools = s.rpc("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 9);
    assert_eq!(s.rpc("ping", json!({}))["result"], json!({}));
    s.send_raw("{não é json");
    assert_eq!(s.read()["error"]["code"], -32700);
    assert_eq!(s.rpc("resources/list", json!({}))["error"]["code"], -32601);

    // ---- search_cards
    let d = s.tool("search_cards", json!({})).unwrap();
    assert_eq!(names(&d)[0], "Sol Ring", "sem argumentos = mais populares");
    let d = s.tool("search_cards", json!({"tags": ["ramp"], "colors": "bg", "limit": 30})).unwrap();
    for c in d["cards"].as_array().unwrap() {
        assert!(c["color_identity"].as_array().unwrap().iter().all(|x| x == "B" || x == "G"), "{c}");
    }
    let d = s.tool("search_cards", json!({"types": ["Instant", "Sorcery"], "tags": ["edict"], "format": "Pauper", "limit": 30})).unwrap();
    let types: String = d["cards"].as_array().unwrap().iter().map(|c| c["type_line"].as_str().unwrap()).collect();
    assert!(types.contains("Instant") && types.contains("Sorcery"), "types é OU");
    let d = s.tool("search_cards", json!({"limit": "5", "cmc_max": "2", "tags": ["removal"], "commander_only": "false"})).unwrap();
    assert_eq!(d["count"], 5, "números como texto");
    assert!(d["cards"].as_array().unwrap().iter().all(|c| c["cmc"].as_f64().unwrap() <= 2.0));
    let e = s.tool("search_cards", json!({"tags": ["haste"]})).unwrap_err();
    assert!(e.contains("haste_enabler"), "sugere a tag certa: {e}");

    // ---- get_card
    for (asked, expected) in [
        ("sol ring", "Sol Ring"),
        ("Thassa’s Oracle", "Thassa's Oracle"),
        ("Lim-Dul's Vault", "Lim-Dûl's Vault"),
        ("Æther Vial", "Aether Vial"),
        ("Fire", "Fire // Ice"),
    ] {
        let d = s.tool("get_card", json!({"name": asked})).unwrap();
        assert_eq!(names(&d).first().map(String::as_str), Some(expected), "get_card({asked})");
    }
    let d = s.tool("get_card", json!({"name": "tortex"})).unwrap();
    assert!(d["suggestions"].to_string().contains("Tortured Existence"));
    let d = s.tool("get_card", json!({"name": "Gurmag the Destroyer"})).unwrap();
    assert!(d["suggestions"][0].as_str().unwrap().starts_with("Gurmag"), "{}", d["suggestions"]);

    // ---- suggest_deck
    let d = s.tool("suggest_deck", json!({"commander": "jarad", "per_section": 3})).unwrap();
    assert!(d["commander"]["name"].as_str().unwrap().starts_with("Jarad"));
    assert!(s.tool("suggest_deck", json!({"commander": "Sol Ring"})).is_err());

    // ---- validate_deck
    let ok = "4 Snuff Out\n4 Cast Down\n52 Swamp\nSideboard\n2 Duress";
    let d = s.tool("validate_deck", json!({"decklist": ok, "format": "pauper"})).unwrap();
    assert_eq!(d["valid"], true, "{}", problems(&d));
    assert_eq!((d["main_count"].as_u64(), d["sideboard_count"].as_u64()), (Some(60), Some(2)));

    let d = s.tool("validate_deck", json!({"decklist": "4 Snuff Out\n56 Swamp\nSB: 2 Duress", "format": "pauper"})).unwrap();
    assert_eq!(d["valid"], true, "{}", problems(&d));
    assert_eq!(d["sideboard_count"], 2, "SB: do MTGO");

    let d = s.tool("validate_deck", json!({"decklist": "4 Lightning Bolt (M10) 146\n1 Sol Ring (C21) 263 *F*\n55 Mountain", "format": "legacy"})).unwrap();
    assert!(d["unknown"].as_array().unwrap().is_empty(), "Arena/Moxfield: {}", d["unknown"]);

    for (list, fmt, must) in [
        ("4 Black Lotus\n56 Island", "vintage", "restrita"),
        ("8 Seven Dwarves\n52 Mountain", "legacy", "Seven Dwarves: 8"),
        ("4 lightning bolt\n4 LIGHTNING BOLT\n52 Mountain", "modern", "Lightning Bolt: 8"),
        ("4 Lightening Bolt\n56 Mountain", "modern", "não encontrada"),
    ] {
        let d = s.tool("validate_deck", json!({"decklist": list, "format": fmt})).unwrap();
        assert!(problems(&d).contains(must), "{list:?}: esperava '{must}' em {}", problems(&d));
    }
    assert!(s.tool("validate_deck", json!({"decklist": "4 Lightning Bolt", "format": "banana"})).unwrap_err().contains("formato desconhecido"));

    let cheap = "1 Sol Ring\n1 Lightning Bolt\n1 Jeska's Will\n96 Mountain";
    let d = s.tool("validate_deck", json!({"decklist": cheap, "format": "commander", "commander": "Krenko, Mob Boss",
        "price_max": 1, "no_game_changers": true})).unwrap();
    let p = problems(&d);
    assert!(p.contains("orçamento") && p.contains("Jeska's Will") && p.contains("Game Changers"), "{p}");
    assert!(!p.contains("Krenko") && !p.contains("Mountain"), "comandante e básicos fora do orçamento: {p}");

    let d = s.tool("validate_deck", json!({"decklist": "1 Counterspell\n98 Mountain", "format": "commander", "commander": "Krenko, Mob Boss"})).unwrap();
    assert!(problems(&d).contains("identidade de cor"), "{}", problems(&d));

    // ---- regras (Comprehensive Rules + rulings)
    if Path::new("data/rules.json").is_file() {
        let d = s.tool("get_rule", json!({"number": "702.19b"})).unwrap();
        assert!(d["rule"]["text"].as_str().unwrap().contains("trample"));
        assert_eq!(d["rule"]["context"], "7 Additional Rules › 702 Keyword Abilities › 702.19 Trample");
        assert!(d["meta"]["pdf"].as_str().unwrap().ends_with("comprehensive-rules.pdf"));

        let d = s.tool("get_rule", json!({"number": "regra 702.19"})).unwrap();
        assert!(d["subrules"].as_array().unwrap().len() >= 3, "sub-regras de 702.19");
        let d = s.tool("get_rule", json!({"number": "Trample"})).unwrap();
        assert!(d["glossary"]["text"].as_str().unwrap().contains("702.19"));
        assert!(s.tool("get_rule", json!({"number": "999.9"})).is_err());

        let d = s.tool("search_rules", json!({"query": "commander damage 21 combat damage loses", "limit": 3})).unwrap();
        let ids: Vec<&str> = d["results"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
        assert!(ids.iter().any(|i| *i == "903.10a" || *i == "704.6c"), "dano de comandante: {ids:?}");

        // achados nos testes da v0.4.0 com o MCP de verdade
        let d = s.tool("search_rules", json!({"query": "imposto de comandante custo adicional zona de comando", "limit": 3})).unwrap();
        assert!(d["results"].to_string().contains("903.8"), "busca em português: {}", d["results"]);
        let d = s.tool("search_rules", json!({"query": "London mulligan put cards on bottom", "limit": 4})).unwrap();
        assert!(d["results"].to_string().contains("\"103.5\""), "glossário traz a regra citada: {}", d["results"]);
        let d = s.tool("get_rule", json!({"number": "702"})).unwrap();
        assert!(d["subrule_count"].as_u64().unwrap() > 150);
        assert_eq!(d["subrules"].as_array().unwrap().len() as u64, d["subrule_count"].as_u64().unwrap(), "sem cortar a lista");
        assert!(d["note"].is_string());
        let d = s.tool("get_rule", json!({"number": "Hexproof"})).unwrap();
        assert_eq!(d["see_rules"][0]["id"], "702.11", "glossário traz a regra");
        let d = s.tool("get_rule", json!({"number": "613.8"})).unwrap();
        assert!(d["subrules"][0].get("context").is_none(), "sub-regra sem contexto repetido");
        assert!(Path::new(d["meta"]["pdf"].as_str().unwrap()).is_absolute(), "caminho do PDF absoluto");

        // pergunta em português com nomes de cartas: os nomes não podem poluir a busca
        let d = s.tool("explain_interaction", json!({
            "cards": ["Rest in Peace", "Gravecrawler"],
            "question": "se Rest in Peace está em campo, posso conjurar Gravecrawler do cemitério?"
        })).unwrap();
        let concepts = d["concept_rules"].to_string();
        assert!(concepts.contains("614.1a") && concepts.contains("601.3"), "conceitos das cartas: {concepts}");
        let related = d["related_rules"].to_string();
        assert!(!related.contains("702.50"), "'Rest in Peace' não pode puxar Epic ('rest of the game'): {related}");

        let d = s.tool("get_rulings", json!({"name": "Blood Moon"})).unwrap();
        assert!(d["card"]["rulings"].to_string().contains("before it can trigger"));

        let d = s.tool("explain_interaction", json!({
            "cards": ["Blood Moon", "Urborg, Tomb of Yawgmoth", "Stinkweed Imp", "Cartinha Inventada"],
            "question": "layers type-changing ability-removing effects dependency"
        })).unwrap();
        assert_eq!(d["cards"].as_array().unwrap().len(), 3);
        assert_eq!(d["unknown"][0]["name"], "Cartinha Inventada");
        let imp = &d["cards"][2];
        let kw: Vec<&str> = imp["keyword_rules"].as_array().unwrap().iter().map(|k| k["rule"].as_str().unwrap()).collect();
        assert!(kw.contains(&"702.52") && kw.contains(&"702.9"), "dredge e flying: {kw:?}");
        let related = d["related_rules"].to_string();
        assert!(related.contains("\"613."), "camadas (613) para a pergunta de mudança de tipo: {related}");
    } else {
        eprintln!("AVISO: data/rules.json não existe; testes de regras pulados.");
    }

    // ---- desempenho: a lista de 1000 linhas usava 11 s antes do índice de nomes
    let big: String = (0..1000).map(|i| format!("1 {}\n", ["Sol Ring", "Dark Ritual"][i % 2])).collect();
    let t = std::time::Instant::now();
    let d = s.tool("validate_deck", json!({"decklist": big, "format": "vintage"})).unwrap();
    assert_eq!(d["main_count"], 1000);
    assert!(t.elapsed().as_secs_f64() < 2.0, "validate_deck lento: {:?}", t.elapsed());
}
