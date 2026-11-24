<div align="center">

<img src="assets/banner.jpg" alt="MTG-RAG" width="100%">

# MTG-RAG

**Busca local sobre todas as ~36 mil cartas de Magic: The Gathering e as regras oficiais do jogo —<br>
classificadas por função, gatilho, alvo e arquétipo, prontas para um LLM montar decks e responder dúvidas de regra.**

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Cartas](https://img.shields.io/badge/cartas-35.892-6f42c1)
![Tags](https://img.shields.io/badge/tags-112-2ea44f)
![Regras](https://img.shields.io/badge/regras-3.340-c2410c)
![Dados](https://img.shields.io/badge/dados-Scryfall%20%2B%20Wizards-1f6feb)
![Offline](https://img.shields.io/badge/roda-100%25%20local-black)

[Instalação](#-instalação) •
[Uso](#-uso) •
[Classificação](#-classificação) •
[Regras](#-regras-do-jogo) •
[Com um LLM / MCP](#-usando-com-um-llm) •
[Desenvolvimento](#-desenvolvimento)

</div>

---

## 🔮 O que faz

- **Todas as cartas, no seu computador.** Baixa o bulk *Oracle Cards* do Scryfall (~25 MB) e gera um dataset local. Depois disso, nada de internet.
- **Classificação pensada para montar deck.** Cada carta recebe tags em 4 categorias — *removal*, *ramp*, *sacrifice_outlet*, *death_value*, *aristocrats*, *reanimator*... — além de legalidade por formato, preço, rank do EDHREC, Game Changer e identidade de cor.
- **Busca híbrida.** Filtros exatos + tags + BM25 com stemming (`creatures` casa com `creature`, `died` com `dies`). Aceita consultas em inglês ou português.
- **Ajuda de deck de Commander.** `deck "<comandante>"` sugere cartas por seção (ramp, compra, remoção, temas do comandante, sinergia, terrenos na cor certa).
- **Regras do jogo.** As *Comprehensive Rules* oficiais (3.340 regras + glossário) e os *rulings* de 20 mil cartas, para responder dúvidas de situação de jogo citando o número da regra. O [PDF das regras](docs/MagicCompRules-20261106.pdf) vem junto, para conferir.
- **Servidor MCP embutido.** `magic_rag mcp` vira ferramenta para Claude e outros LLMs: buscar, ver carta, sugerir e **validar** decks, e **explicar interações** com as regras oficiais.
- **Saída JSON** para mandar direto a um LLM (RAG de verdade: ele só responde com cartas reais).

## 📦 Instalação

### Opção 1 — executável (Windows)

1. Baixe o `magic_rag.exe` na página de [**Releases**](../../releases).
2. Coloque numa pasta e **dê dois cliques**. Ele baixa e prepara cartas, rulings e regras sozinho:

```
== Instalando cartas, rulings e regras do Magic em data
baixando cartas (Oracle Cards) (~26 MB)...
baixando rulings das cartas (~6 MB)...
baixando Comprehensive Rules (MagicCompRules 20261106.txt)...
35892 cartas salvas em data\cards.json (2.3s)
3340 regras e 744 verbetes de glossário (em vigor desde November 6, 2026)
80928 rulings de 20387 cartas
Pronto!
```

Ou pelo terminal: `.\magic_rag.exe install`.

### Opção 2 — a partir do código

Precisa do [Rust](https://rustup.rs).

```bash
git clone https://github.com/Yelllowww/MTG-RAG.git
cd MTG-RAG
cargo build --release
./target/release/magic_rag install
```

> O dataset **não fica no repositório** — é sempre gerado do Scryfall, então você tem as cartas mais novas.
> Para atualizar depois de uma coleção nova, rode `magic_rag install` de novo.

## 🔎 Uso

```powershell
$m = ".\magic_rag.exe"

# busca livre (inglês ou português) + filtros
& $m search "whenever creature dies draw card" --boost card_draw --format commander
& $m search "" --tag removal --colors W,B --cmc-max 2 --format commander
& $m search "" --tag death_value --type Creature --cmc-max 1 --format pauper --colors B
& $m search "" --type Instant --type Sorcery --tag edict --format pauper
& $m search "" --commander --tag theme:tokens --colors W,G
& $m search "..." --json                          # para mandar ao LLM

# ficha completa de uma carta
& $m card "Jarad, Golgari Lich Lord"

# esqueleto de deck de Commander
& $m deck "Krenko, Mob Boss" --per 10 --budget 5 --no-game-changers

# taxonomia e métricas
& $m tags
& $m stats
```

<details>
<summary><b>Todos os filtros do <code>search</code></b></summary>

| filtro | o que faz |
|---|---|
| `--colors W,U` | identidade de cor permitida (`C` = só incolor) |
| `--has-colors W,U` | identidade precisa conter essas cores |
| `--cmc-min N` / `--cmc-max N` | faixa de custo |
| `--type T` | tipo; repetir = "ou" (`--type Instant --type Sorcery`) |
| `--subtype S` / `--tribe S` | subtipo exato / tipo de criatura que a carta é **ou menciona** |
| `--rarity R` / `--name TXT` | raridade / nome contém |
| `--tag T` / `--any-tag T` / `--not-tag T` | exige todas / ao menos uma / exclui |
| `--boost T` | só aumenta a nota |
| `--format F` | legal no formato (commander, pauper, modern...) |
| `--commander` | só cartas que podem ser comandante |
| `--instant-speed` | instantâneos e cartas com Flash |
| `--price-max USD` | preço máximo |
| `--paper` | esconde cartas só digitais (Alchemy) e "Un-" |
| `--no-game-changers` | esconde Game Changers (brackets de Commander) |
| `--sort S` | relevance, popularity, cmc, price, name, newest |
| `--top N` / `--json` | quantidade / saída JSON |

Tags com nome repetido podem ser qualificadas: `--tag theme:lifegain` (deck de ganho de vida) × `--tag function:lifegain` (carta que ganha vida).

</details>

## 🏷️ Classificação

| categoria | para que serve | exemplos |
|---|---|---|
| **function** (58) | o papel da carta no deck | `removal` `edict` `board_wipe` `counterspell` `card_draw` `tutor` `ramp` `mana_rock` `fixing` `token_maker` `reanimation` `recurring` `sacrifice_outlet` `death_value` `fling` `free_spell`... |
| **trigger** (18) | quando ela age | `etb` `dies` `attacks` `cast_spell` `landfall` `upkeep` `activated`... |
| **target** (12) | o que ela mira | `creature` `player` `spell` `any_target` `graveyard_card`... |
| **theme** (24) | arquétipo de deck | `aristocrats` `tokens` `counters` `spellslinger` `voltron` `reanimator` `graveyard` `stax` `storm` `tribal`... |

Algumas tags feitas especialmente para montar deck:

| tag | significa | exemplos |
|---|---|---|
| `death_value` | gera valor ao morrer (fodder de sacrifício) | Nested Shambler, Shambling Ghast |
| `recurring` | volta sozinha (undying, persist, dredge…) | Young Wolf, Stinkweed Imp, Gravecrawler |
| `fling` | dano igual ao poder da criatura sacrificada | Fling, Jarad, Rite of Consumption |
| `edict` | oponente sacrifica (passa por hexproof) | Accursed Marauder, Innocent Blood |
| `discard_outlet` | descartar como custo | Tortured Existence |
| `hand_disruption` | mexe na mão do oponente | Duress, Chittering Rats |
| `free_spell` | custo alternativo grátis | Snuff Out, Force of Will |
| `scaling` | poder cresce com uma contagem | Lord of Extinction |

A classificação é feita por regras (regex + lógica) sobre o texto oracle normalizado:
sem texto de lembrete, com o nome da carta trocado por `~` e `creature card` virando
`creature_card` (para não confundir "exile target creature" com "exile target creature card from a graveyard").

## 📖 Regras do jogo

Para dúvidas de **situação de jogo** ("Blood Moon desliga o Urborg?", "dredge com 3 cartas no grimório?",
"dano de comandante roubado conta?"), o MTG-RAG usa só fontes oficiais:

| fonte | o que responde | de onde vem |
|---|---|---|
| **Comprehensive Rules** | como o jogo funciona: camadas, pilha, combate, cada keyword | [Wizards](https://magic.wizards.com/en/rules), baixadas pelo `install` |
| **Rulings** | esclarecimentos oficiais sobre cartas específicas | [Scryfall](https://scryfall.com/docs/api/bulk-data) (Wizards + Scryfall) |

```powershell
& $m rule 702.19                 # trample: contexto, sub-regras e exemplos
& $m rule 613.1                  # camadas
& $m rules "commander damage 21" # busca por texto (melhor em inglês, com termos de regra)
& $m rulings "Blood Moon"        # rulings oficiais da carta
```

**Conferir uma regra pelo número:** o PDF oficial está em
[`docs/MagicCompRules-20261106.pdf`](docs/MagicCompRules-20261106.pdf) (e o `install` baixa a versão mais nova
em `data/comprehensive-rules.pdf`). Abra e procure pelo número (Ctrl+F `702.19b`).

Com o servidor MCP, o LLM chama `explain_interaction` com as cartas e a pergunta (em português ou inglês) e responde
**citando os números das regras e os rulings**. A ferramenta também aponta os conceitos de regra que o texto das cartas
já indica, por exemplo *"instead"* (efeito de substituição, 614.1a) ou *"can't"* (101.2). Em três rodadas de teste com
17 dúvidas clássicas, de trample + deathtouch até Humility + Opalescence (camadas) e Platinum Angel + Abyssal
Persecutor, as 17 respostas vieram certas e com as regras corretas.

> As regras são atualizadas pela Wizards a cada coleção (~4 vezes por ano). Rode `magic_rag install` de novo para
> atualizar; o arquivo em `docs/` é a versão de 06/11/2026.

## 🤖 Usando com um LLM

### Servidor MCP (recomendado)

O executável também é um servidor [MCP](https://modelcontextprotocol.io): `magic_rag mcp`.
Qualquer cliente MCP (Claude Code, Claude Desktop, Cursor...) passa a ter estas 9 ferramentas:

| ferramenta | o que faz |
|---|---|
| `search_cards` | busca com texto + todos os filtros e tags do `search` |
| `get_card` | ficha completa; ignora acentos e apóstrofos (*Lim-Dul's Vault*, *Thassa’s Oracle*) e, se o nome não existir, sugere parecidos (*"tortex" → Tortured Existence*) |
| `suggest_deck` | sugestões por seção para um comandante |
| `validate_deck` | confere a lista: cartas inexistentes (com sugestão), legalidade, cópias (inclusive restritas e Seven Dwarves), identidade de cor, contagem, Game Changers, orçamento por carta, preço e curva. Aceita listas do Arena, MTGO (`SB:`) e Moxfield |
| `explain_interaction` | **dúvida de regra**: junta texto, rulings oficiais e regras das keywords de cada carta, mais as regras relevantes para a pergunta |
| `search_rules` | busca nas Comprehensive Rules e no glossário (aceita número: `702.19b`) |
| `get_rule` | uma regra pelo número, com contexto, sub-regras e exemplos |
| `get_rulings` | rulings oficiais de uma carta |
| `list_tags` | a taxonomia com significado e quantidade de cartas |

As cartas e o índice ficam carregados na memória: depois do primeiro segundo, cada consulta é instantânea.

**Claude Code:**

```powershell
claude mcp add mtg-rag --scope user -- "C:\caminho\para\magic_rag.exe" mcp
```

Quem clonar este repositório já recebe um `.mcp.json` apontando para `./target/release/magic_rag.exe`:
basta `cargo build --release`, `magic_rag install` e aprovar o servidor `mtg-rag` ao abrir o `claude` na pasta.

**Claude Desktop** (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "mtg-rag": { "command": "C:\\caminho\\para\\magic_rag.exe", "args": ["mcp"] }
  }
}
```

> Rode `magic_rag install` antes, para baixar cartas, rulings e regras. O servidor acha a pasta `data/` ao lado do executável.

### Pela linha de comando

```
Você ──(pergunta em português)──▶ LLM ──(filtros + tags)──▶ magic_rag search --json
                                   ▲                                  │
                                   └──────(só cartas reais)───────────┘
```

1. Você pergunta: *"monta um deck pauper mono black de sacrifício"*.
2. O LLM traduz em consultas: `search "" --tag death_value --format pauper --colors B --json`.
3. O programa devolve as cartas **reais**, com texto, legalidade e preço (Retrieval).
4. O LLM responde lendo só essas cartas (Generation) — sem inventar carta nem errar legalidade.

## 🛠️ Desenvolvimento

```
src/
  main.rs     linha de comando (install / fetch / build / search / card / deck / tags / stats / mcp)
  mcp.rs      servidor MCP (JSON-RPC 2.0 via stdio)
  rules.rs    Comprehensive Rules (parser, hierarquia, busca) e rulings por carta
  fetch.rs    baixa o bulk do Scryfall
  build.rs    bulk -> data/cards.json
  card.rs     structs RawCard (Scryfall) e Card (dataset)
  tags.rs     taxonomia, regras e testes de classificação   <- edite aqui
  search.rs   busca híbrida: filtros + tags + BM25 com stemming
  deck.rs     sugestões por categoria para Commander
tests/
  golden_cards.json   regressão: 68 cartas com as tags obrigatórias e proibidas
  mcp_e2e.rs          sobe o servidor MCP real e testa as 9 ferramentas (pulado sem data/)
docs/
  MagicCompRules-20261106.pdf   Comprehensive Rules oficiais (Wizards), para conferir uma regra pelo número
```

**Ao mudar uma regra de classificação:**

1. `cargo test --release` — o teste `golden_cards_regression` lista **todas** as cartas conhecidas que quebraram. Para cobrir um caso novo, acrescente a carta em `tests/golden_cards.json` (`must` = tags obrigatórias, `must_not` = proibidas).
2. `magic_rag stats --json > antes.json` → `magic_rag build` → `magic_rag stats --json > depois.json` e compare a cobertura e as contagens por tag.

---

<div align="center">
<sub>
Dados das cartas: <a href="https://scryfall.com/docs/api/bulk-data">Scryfall</a>.
Projeto não oficial, sem afiliação com a Wizards of the Coast.<br>
Magic: The Gathering, os nomes, textos e artes das cartas (inclusive a do banner) e as Comprehensive Rules são © Wizards of the Coast LLC.
</sub>
</div>
