# План: інтеграція карт у Rust-рушій `tavernlab-sim`

Дата: 2026-08-25. Базовий коміт: `8b82dbc` (main). Усе нижче перевірено на
цьому чекауті: `cargo test --workspace` зелений (110 core + 161 cards + 12 json
= 283), `cargo run -p xtask -- cards` перегенеровує `table.rs` без diff — дані
й таблиця в синхроні.

## 0. Що прийшло в репозиторій

- `991f09f` — перший коміт Rust-рушія `tavernlab-sim/` (workspace: `json`,
  `core`, `cli`, `xtask`). 112k рядків, з них 100k — згенерований
  `core/src/cards/table.rs`. Коміт-повідомлення описує: виправлення пайплайну
  (spell_damage/overload/dormant читаються з полів корпусу, а не з тексту;
  ключові слова для карт, яких ще немає в CardDefs, беруться зі списку API;
  `MECHANIC_ALIASES`), `childIds` → `CHILD_SLICES/CHILD_IDS`, `token()` як
  `const fn` замість ручного списку `REFERENCED_TOKENS`, +33 карти
  (164 → 198 Standard), Burn Mage грає від початку до кінця.
- `8b82dbc` — README (джерело даних тепер «офіційні джерела Blizzard» +
  `scripts/build_cards.py`) і `rust_impl.txt` (157 назв).

**`rust_impl.txt` — застарілий зріз.** Це строга підмножина виводу
`tavernsim implemented` (зараз 198 назв; 41 карта реалізована після зрізу).
Не використовувати як джерело істини; або перегенерувати
(`cargo run -q -p tavernsim -- implemented > rust_impl.txt`), або видалити.

## 1. Поточний стан (виміряно)

| метрика | значення |
|---|---|
| Standard: реалізовано / deckable | 198 / 1210 (16.4%), 7 з них APPROXIMATE |
| Wild | 664 / 7017 |
| Слоти 12 мета-колод (`hs2/meta_decks_2026.json`), що резолвляться | **224 / 350** |
| Колод, що грають повністю | 1 (Burn Mage) |
| Python `hs2/impls.py` реалізовано (Standard, collectible) | 265; з них **106 ще немає в Rust** |
| Карт мета-колод, яких бракує в Rust | **79 різних**; 73 з них уже є в Python, 6 — ні |

Покриття по колодах (слоти OK / всього; карт бракує):

| колода | OK | бракує |
|---|---|---|
| Burn Mage | 30/30 | 0 |
| UUB Egg Death Knight | 24/30 | 5 |
| Quest Demon Hunter | 22/30 | 7 |
| Raza Demon Hunter | 21/30 | 6 |
| Attack Druid | 20/30 | 7 |
| Quest Hunter | 20/30 | 7 |
| Zee Shaman | 20/30 | 9 |
| Fatigue Paladin | 18/30 | 8 (+ sideboard: Prize Vendor ×1 для Beatrix) |
| Herald Warlock | 16/30 | 9 |
| Dragon Warrior | 16/30 | 8 |
| Herald Rogue | 10/30 | 16 |
| Thief Priest | 7/20 | 8 (колода на 20 карт — Azalina) |

Повний список карт по колодах з текстами — у §5.

## 2. Як влаштований рушій (шпаргалка для агента)

Читати перед першою картою: `core/src/lib.rs` (головний інваріант: `Game` —
значення фіксованого розміру, клонується `memcpy`; жодних `Vec/Box/String` у
живому стані; тест `a_game_is_small_enough_to_copy_freely` у `state.rs` пінить `size_of::<Game>() < 2048` байт), `core/src/cards/behaviour.rs`
(шапка + `Behaviour`, `Ctx`, `TargetSpec`), `core/src/effects.rs` (словник
дієслів), `core/src/events.rs` (`Event`, `TriggerCtx`).

- **Рядок = карта.** `BEHAVIOURS: &[Behaviour]` у `behaviour.rs`, ключ — **назва**
  (чіпляється до всіх принтів). Конструктори: `spell(name, T::…, |g, c| …)`,
  `battlecry(…)`, `deathrattle(name, …)`, `trigger(name, |g, c| …)`,
  `aura(name, |ss, sl, ts, tl, m| (atk, hp))`, `secret(name, |g, owner, ev| bool)`,
  `choose(name, &[m(T::…, f), …])`, повний `c(name, target, spell, battlecry,
  deathrattle, trigger, aura, secret, choose, cost_delta)`.
- **Замикання без захоплень** → `fn`-покажчик; таблиця статична.
- **Дієслова** — методи `Game` в `effects.rs` (`spell_damage`, `damage_area`,
  `draw_cards`, `summon_token/child/random_child`, `buff`, `grant`, `destroy`,
  `silence`, `bounce`, `discover`, `discover_from_deck`, `equip`, `gain_corpses`,
  `spend_corpses`, `herald`, `summon_dormant`, `set_attack/health`, `transform`,
  `freeze_area`, `retrigger_friendly_deathrattles`, `kindred`, `holding_race`,
  `combo_active`, …). Бракує дієслова — **додати в `effects.rs`**, не лізти в
  стан із рядка карти.
- **Токени:** `mod tokens { pub const X: CardId = token("ID"); }` — резолвиться
  на етапі компіляції, невідомий id ламає збірку. Або `c.card.summonable_children()`
  (діти з `childIds` API) — для «summon a 1/1 Murloc Scout» тощо.
- **`is_implemented`:** є рядок, або міньйон/зброя з бітом `TEXT_UNDERSTOOD`
  (текст = тільки ключові слова). Location / Hero / HeroPower без коду —
  нереалізовані.
- **Location:** активна здатність живе в хуку `spell` (`game.rs::use_location`).
- **Hero Power:** матчиться по назві в `game.rs::use_hero_power`;
  `Player.hero_power: CardId` можна підмінити.
- **Секрети:** повертають `bool` «спрацював» — інакше залишаться назавжди.
- **Aura:** чиста функція від позицій (`ss, sl, ts, tl, target`) — навмисно не
  бачить решту гри.
- **cost_delta:** `fn(&Game, Side, hand_idx) -> i16`, читається щоразу;
  per-copy знижка — `HandCard.cost_delta`.
- **Стан гравця, що вже є:** `corpses`, `herald`, `next_spell_discount`,
  `cards_played_turn`, `spells_cast_turn`, `spell_power_bonus`,
  `played_races_turn/last` (Kindred), `overload_now/next`, `deaths_this_turn`
  (на `Game`), `secrets`, `deck: Inline<CardId, 60>` (верх = кінець масиву,
  тож «на дно колоди» = `insert(0, …)`).
- **`APPROXIMATE: &[(&str, &str)]`** — контракт: частково реалізована карта
  **слабша за надруковану, ніколи не сильніша**; нотатка каже, чого бракує.
  Виняток (Archmage Kalec) названо явно. Кожна нова апроксимація — туди.
- **Тести:** `core/tests/cards.rs` — по тесту на карту через `Fix`
  (`Fix::new().board(FOE, &["Boulderfist Ogre"])`, `f.play("Fireball",
  foe_minion(0))`, `f.play_mode(name, k, …)`, `f.deck(&[…])`); стверджувати
  **дошку/руку/HP**, а не переможця. Інваріанти в `behaviour.rs::tests`: назва
  існує в корпусі; без дублікатів; спел має `spell` або `secret`; battlecry/
  deathrattle збігаються з друкованим ключовим словом; choose ≥ 2 режими.
- **Дані:** `scripts/build_cards.py --include-carddefs-only` →
  `hs2/build_data.py cards_merged.json --format both` →
  `cargo run -p xtask -- cards` → `table.rs`. **`table.rs` руками не правити.**
- **CLI:** `cargo run -q -p tavernsim -- coverage | implemented [wild] | bench | matrix | demo`.
- **Python `hs2/impls.py`** — референс семантики, не шаблон. Він писався проти
  корпусу з трьома даними-багами (spell damage, dormant, keywords) — кожну
  портовану карту звіряти з текстом у корпусі (`INFO[..].text`) і, де сумнів,
  з hearthstone.wiki.gg.

## 2a. Дані: що дає `scripts/build_cards.py` і що з цього випливає

`build_cards.py` (коміти `4099d7a`, `04e670e`) будує корпус так: **хребет —
живий Blizzard card-library API** (лише карти, що реально існують у грі),
**CardDefs.xml** (`hearthstone` + `hearthstone-data`) лише дозаповнює те, чого
API не віддає: рядковий id (`EX1_298`), `mechanics`/`referencedMechanics`,
spell damage, overload, hero power. Ключ злиття — `dbfId`. Наслідки для
інтеграції карт:

- **Пул 1210 deckable Standard — справжній.** Нічого, чого немає в живій грі,
  у таблицю не потрапляє; `curve_deck`, Discover-пул і `coverage` рахують по
  реальних картах. Портрети героїв більше не «виграють» у справжніх карт
  (тест `name_lookup_never_resolves_to_a_hero_portrait`).
- **`--include-carddefs-only` для Rust обов'язковий.** Без нього в корпусі
  немає токенів/hero power, і `token("skele21")` тощо ламають збірку на
  етапі компіляції. Повна команда для регенерації:
  `python3 scripts/build_cards.py --include-carddefs-only && python3 hs2/build_data.py cards_merged.json --format both && (cd tavernlab-sim && cargo run -p xtask -- cards)`.
- **`childIds` з API** → `child` у `standard_cards.json` → `CHILD_SLICES/CHILD_IDS`
  → `CardId::children()/summonable_children()`. Це єдине джерело зв'язку
  «карта → її токени»; саме тому «summon a 1/1 Murloc Scout» — лукап, а не
  id, набраний руками. Для нових карт (Egg of Khelos, Portal-ланцюжок, Twilight
  Egg, Nespirah) спершу дивитись `children()`, і лише якщо там порожньо —
  `tokens::` + `token("ID")`.
- **29 карт «новіші за CardDefs»** (міні-сет: Blastpowder Engineer, Captain
  Crowley, Godfather Kazakus, Mathias Shaw, Sinful Steed, Land Ho!, …). У них
  `id` = slug (`sinful-steed`), ключові слова — зі списку API, `TEXT_UNDERSTOOD`
  не ставиться, тому навіть ванільні тіла серед них — «нереалізовані».
  Жодна з них не входить у 79 карт мета-беклогу, тож на фази 1–5 це не
  впливає. Дешевий виграш окремо: оновити `hearthstone-data`
  (`pip install -U hearthstone hearthstone-data`), перебудувати корпус — ці 29
  отримають справжні id і mechanics, частина стане реалізованою без рядка.
  **Після оновлення обов'язково перегенерувати `table.rs` окремим комітом**, бо
  зміняться id (slug → `XXX_123`) і будь-який `token("slug")` перестане
  резолвитись.
- **`hs2/build_data.py` не пропускає `sideboard`, `deckSizeMod`, `runeCost`,
  `bundledCardIds`** — API їх віддає, `build_cards.py` кладе в
  `cards_merged.json`, але до `standard_cards.json` вони не доходять. Для
  G10 (Commander Beatrix — sideboard 10 копій; Azalina — `deckSizeMod`) і
  для рун DK (валідатор колод) їх треба пропустити через `build_data.py`
  → `xtask` → `CardDef`/`CardInfo`. Це зміна даних → окремий коміт
  «regenerate table» (див. §6 п.7).
- **`cards_merged.json` і `blizzard_api_cache.json` відсутні в цьому чекауті
  й не в `.gitignore`** (там лише старий `cards.json`). Корпус збирався на
  іншій машині (Windows, див. коментар про Smart App Control у
  `core/Cargo.toml`). Додати обидва до `.gitignore`, щоб агент випадково не
  закомітив 9+ МБ; для відтворення на цій машині — `build_cards.py` без
  `--offline` (мережа) або принести кеш. На цьому чекауті встановлено лише
  `hearthstone 9.20.10`, **`hearthstone-data` (CardDefs.xml) немає** — без
  нього `build_cards.py` падає на `hearthstone.cardxml.load`; ставити
  `pip install hearthstone hearthstone-data requests` перед першим запуском.
- **Текст карти для 6 карт, яких немає в Python** (Axe of Cenarius, First
  Portal to Argus, The Kingslayers, Atiesh, Karazhan, King Llane) — брати з
  `INFO[id].text` (це текст API); там же `children()` для Portal-ланцюжка.

## 3. Прогалини рушія, відсортовані за кількістю мета-слотів, які відкривають

Нових механізмів у ядрі бракує небагато, але вони блокують більшість карт.
Кожен — окрема невелика зміна в `state.rs`/`game.rs`/`effects.rs` + тест
рівня правил.

| # | механізм | що саме додати | карти (слотів) |
|---|---|---|---|
| G1 | **Позначки на карті в руці** («while holding this», «discard *it*») | `HandCard.marks: u8` (біти: `PLAYED_MINION_WHILE_HELD`, `PLAYED_OPP_COPY_WHILE_HELD`, `PLAYED_HIGHER_COST_WHILE_HELD`, `MANA_25_SPENT`, `DRAWN_BY_PLATYSAUR`…) + оновлення в `apply()` після кожного розіграшу; `Game` лишається `Copy` | Platysaur (4), Ebb and Flow (2), Mind Sweeper (2), Unshackle Soul (2), Shaladrassil (2), Cultist Map (2), Merithra (1) ≈ **15** |
| G2 | **Відкладені ефекти** (start/end of *next* turn, N ходів) | `Player.pending: Inline<Pending, 4>` де `Pending { kind: u8, turns_left: u8, amount: i16 }`; обробка в `begin_turn/end_turn`; kinds: `TempCrystal`, `SummonToken(CardId)`, `HeroDamage`, `SpellTaxOpp`, `DieAtEndOfTurn(slot)`, `ReturnControl` | Acceleration Aura (4), Sigil of the Seas (2), Cursed Chains (2), Rotten Apple (1), Cult Neophyte (1), Ursol (1) ≈ **11** + прибирає Soulrest Ceremony з APPROXIMATE |
| G3 | **Hero-карти** (`Kind::Hero`) | у `legal_actions/apply`: оплатити, `armor` з `def.armor`, замінити `hero_power` на дитину-HeroPower (`children()`), викликати `battlecry` | Deathwing, Worldbreaker (3); потрібна для всіх hero-карт у пулі 1210 |
| G4 | **Colossal** | при вході на дошку (не лише з руки) призвати appendages з `summonable_children()` ліворуч/праворуч; поки що прийнятно як battlecry-апроксимація (слабше) | Al'Akir (1), Wickerfang (1), Sinestra (1), Vulcanos (порт), Ultraxion «Herald your Colossal» (3) ≈ **6** |
| G5 | **Start of Game** хук | нове поле `Behaviour.start_of_game: Option<Effect>` (усі конструктори/`c(...)` оновити — це позиційний запис, компілятор підкаже) + виклик у `Game::start` після муллігана | Broxigar (2), Mug'Zee (1), Chainbreaker Hogger (1), Godfrey (1), King Llane (1) ≈ **6** |
| G6 | **Quest / Sidequest** | `Player.quest: Option<(CardId, progress: u8)>`, прогрес через `fire()`; нагорода — `give_token`; UI-слот секрета не займати | The Food Chain (1), Unleash the Colossus (1), Storm the Gates (1) — 3 слоти, але це **ідентичність двох колод** |
| G7 | **Примусова атака** (`attack_with(attacker, defender)`) | винести бойову частину `apply(Attack)` у дієслово, що ігнорує «вже атакував»/Taunt | Emergency Surgery (1), Spire of Solitude (2), Temporal Traveler (порт) |
| G8 | **Hero Divine Shield / підміна та апгрейд hero power** | `Player.hero_divine_shield: bool`; `Player.hero_power_level: u8` для Imbue і Collapsing Star; другий hero power для Thal'ena — `Option<CardId>` | Hardlight Protector (2), Soul Immolation (2), Lunarwing Messenger (2), Blood Doctor Thal'ena (1) ≈ **7** |
| G9 | **Thief-лічильник** («played a copy of an opponent's card») | `Player.opp_class_cards_played: u8` + біт у G1 | Mind Sweeper, Unshackle Soul, Sinestra (перетинається з G1) |
| G10 | **Правила побудови колоди** | читати `sideboard` з JSON (Beatrix: 10 копій 2-дропа), 40 HP / 20+20 карт (Azalina), 40-карт (Beatrix) — у `Game::new`/завантажувачі колод | Commander Beatrix (1), Azalina (1) |
| G11 | **Dark Gifts** | окремий механізм; **спочатку апроксимувати** (Discover без дару — слабше) і внести в APPROXIMATE | Darkrider (2), Shadowflame Suffusion (2), Nightmare Fuel (2) |
| G12 | **Дно колоди** | тривіально: `deck.insert(0, …)` / читання `deck[..3]` — можливо, лише дієслово `put_on_bottom` | Imp Gang Stooge (2), Annihilation (2), King Llane, Cosmic Manifestations (зняти з APPROXIMATE) |

Свідомо **не робити** зараз (мала віддача / велика складність): Toreth
(лічильник DS-хітів — апроксимувати як звичайний DS), Elise the Navigator
(«craft a custom location» — апроксимувати ванільним тілом 4/…), Bashana
(«carve spells» — три голі Treants), Irida (Void), Shadow of Demise
(тригер із руки), Tiny Pal (фіксований боєприпас), Dreambound Raptor.
Усі — в APPROXIMATE з нотаткою, бо вони *слабші* за оригінал.

## 4. Порядок робіт (фази = коміти)

Кожна фаза завершується: `cargo test --workspace` зелений, `cargo run -q -p
tavernsim -- coverage` і `gauntlet` у коміт-повідомленні з числами «було →
стало» (Standard N/1210, слоти M/350), нові APPROXIMATE-записи перелічені.

### Фаза 0 — інструмент вимірювання (½ дня)

1. Команда `tavernsim gauntlet [path]`: читати `hs2/meta_decks_2026.json`
   через крейт `tavernlab-json` (уже в workspace, без залежностей), резолвити
   назви через `by_name`, друкувати по колоді «OK/усього» і список
   нереалізованих карт із текстом; підсумок «224/350». Ураховувати
   `sideboard`. Це замінює ручний підрахунок, з якого взято число в коміті.
2. Тест у `core/tests/` (або `deck.rs`), що пінить «слотів резолвиться ≥ 224»
   — щоб число не могло тихо впасти (за зразком
   `most_classes_can_field_a_curve_deck`).
3. `rust_impl.txt`: перегенерувати або видалити (див. §0).
4. `.gitignore`: додати `cards_merged.json`, `blizzard_api_cache.json` (§2a).
5. (опційно, окремий коміт) `pip install -U hearthstone hearthstone-data` →
   повний пайплайн §2a → `cargo test`; очікується, що 29 slug-карт отримають
   справжні id, а `coverage` трохи зросте без жодного рядка в `behaviour.rs`.

### Фаза 1 — карти без нових механізмів (порт із Python, ~25 карт, +~30 слотів)

Тільки існуючі дієслова (плюс, можливо, 1–2 дрібних: `put_on_bottom`,
`discover_from_opponent_hand`, `summon_random_where(pred)`):

- **DK:** Staff of the Endbringer (weapon deathrattle — перевірити, що
  `game.rs::sweep_deaths` / знищення зброї викликає `deathrattle` для зброї;
  якщо ні — це мікро-механізм), Emergency Surgery ← G7 (можна відкласти).
- **Druid:** Spiderling (тригер `TurnStart` → `hero_bonus_atk += 1`; перевірити
  скидання наприкінці ходу).
- **Hunter:** Guard Dog (`summon_random_of_cost` + фільтр DEATHRATTLE), Earthen
  Roar (`set_health` + `holding_race(DRAGON)` → другу ціль обрати
  детерміновано: найбільша атака), Cower in Fear (3 dmg + `next_beast_discount`
  за зразком `next_spell_discount`).
- **Paladin:** Judgment (PREPARE-біт уже є; `set_attack/set_health` на всіх),
  Twilight Egg (токен-Whelp з тригером росту — окремий рядок для токена),
  Soothsayer (heal 6 + `summon_random_of_cost(6)`), Hardlight Protector ← G8
  (поки heal 3, DS героя — апроксимація).
- **Priest:** Intertwined Fate, Deja Vu (`discover_from_deck` + нове дієслово
  для руки/колоди суперника), Soothsayer.
- **Rogue:** Opu the Unseen (викликати `spell` рядка «Fan of Knives» на
  battlecry/combo/deathrattle), Agent of the Old Ones (перетворити
  найдорожчу/найгіршу карту в руці на `tokens::COIN`), Naralex (`cost_delta`:
  Dragon і `!played_races_turn.any(DRAGON)` → −(cost−1)), Cultist Map ← G1,
  Deja Vu.
- **Shaman:** Getaway Hogdriver (`draw_cards` має повернути, що витягнуто —
  дрібна зміна сигнатури або нове `draw_and_report`), Platysaur ← G1.
- **Warlock:** Imp Gang Stooge, Annihilation ← G12, Cursed Catacombs
  (`discover_from_deck`; «Temporary» — апроксимувати без тимчасовості = карта
  *сильніша*? Ні: Temporary означає «згорає в кінці ходу», без неї карта
  сильніша → або реалізувати через `HandCard.locked_turn`-подібне поле, або
  в APPROXIMATE з явною позначкою «reads stronger»), Eredar Deceptor (тригер
  `CardDrawn` → 1/1 Demon Rush із `summonable_children`).
- **Warrior:** Brood Keeper (`holding_race(DRAGON)` → `equip(token)`), Stadium
  Announcer (обидва `equip` випадкової зброї з `discover_pool`-подібного
  фільтра; Rewind ігнорувати як у Python), Erupting Volcano (Location:
  `damage_split`; «Fire spell this turn» — додати `Player.schools_cast_turn: u8`
  бітами), Torch (звірити з Python: скільки шкоди; «return with excess» —
  `give_token` себе з `cost_delta`), Darkrider / Shadowflame Suffusion ← G11
  апроксимація.
- **DH:** Dark Bribe (draw 3, віддати найдешевшу — детерміновано), Sigil of the
  Seas ← G2.
- **Порт-беклог поза мета-колодами** (з 106; беруться безкоштовно тими самими
  дієсловами): Arcane Missiles, Avatar of Destruction, Best in Shell, Cairne
  Bloodhoof, Chaos Strike, Cinderfin, Death Strike, Demonic Assault, Devouring
  Plague, Eternal Bloodpetal, Felrattler, First Flame, Land Ho!, Living
  Paradox, Mountain Bear, Reluctant Wrangler, Remorseless Winter, Sewer Imp,
  Skyscreamer Eggs, Static Shock, Tankgineer, Unleash the Crocolisks, Void
  Shard, Voidlord, Winterspring Whelp, Raincaller, Time-Twisted Seer
  (умовний Spell Damage — поле `Permanent.spell_damage` є), Unstable
  Spellcaster, Spellweaver's Brilliance (`cost_delta`), Dark Iron Harbinger,
  Smoldering Grove.

### Фаза 2 — G1 + G2 + G12 (два малі механізми ядра, +~26 слотів)

Після них: Platysaur, Ebb and Flow, Mind Sweeper/Unshackle Soul (з G9),
Shaladrassil (потрібні 5 Dream-карт як рядки: Dream, Nightmare, Ysera Awakens,
Laughing Sister, Emerald Drake — клас `Class::Dream` у корпусі є;
corrupt-версії — в APPROXIMATE, як у Python), Cultist Map, Merithra,
Acceleration Aura, Sigil of the Seas, Cursed Chains (+ дієслово зміни
контролю `take_control(target, side)` з поверненням через `pending`), Rotten
Apple, Cult Neophyte, Ursol (апроксимація: скастити один раз), Soulrest
Ceremony (зняти з APPROXIMATE), Cosmic Manifestations (зняти).

Перевірити, що `a_game_is_small_enough_to_copy_freely` (`Game` < 2 КБ) проходить; якщо
`Inline<Pending, 4>` на гравця його ламає — зменшити до 3 або упакувати.

### Фаза 3 — G3 + G4 + G8 (Herald-колоди та Raza DH, +~15 слотів)

Deathwing, Worldbreaker (hero-карта + Cataclysm/Cataclysms — Python
`_deathwing_bc`, herald-лічильник уже в `Player.herald`), Ultraxion (herald +
`cost_delta` на Deathwing у руці через `HandCard.cost_delta`), Al'Akir,
Wickerfang, Sinestra, Vulcanos, Soul Immolation (`hero_power = Collapsing
Star`, рівень шкоди), Lunarwing Messenger (Imbue), Blood Doctor Thal'ena,
Hardlight Protector (повна версія), Soldier of Al'Akir (зняти з APPROXIMATE:
дати аурі змогу читати `herald` — або лишити).

### Фаза 4 — G5 + G6 + G7 + пакет Argus (Quest DH / Quest Hunter / Raza DH до 30/30)

Start of Game: Broxigar, Mug'Zee, Chainbreaker Hogger, Godfrey, King Llane
(+ Garona), Warptooth (тригер «4 friendly damaged this turn» — лічильник на
`Game` за зразком `deaths_this_turn`). Квести: The Food Chain (+ Shokk), Unleash
the Colossus (+ Gorishi Colossus), Storm the Gates (+ Zombeast — апроксимувати
фіксованим). **Не в Python, писати з тексту**: Axe of Cenarius, First Portal
to Argus (ланцюжок Portals — токени-діти), The Kingslayers, Atiesh the
Greatstaff, Karazhan the Sanctum, King Llane. Emergency Surgery, Spire of
Solitude, Temporal Traveler через G7. Nespirah, Enthralled (Location +
тригер на Fel-спел + deathrattle-токен).

### Фаза 5 — G10 і хвіст (Thief Priest, Fatigue Paladin)

Commander Beatrix (sideboard → 10 копій), Azalina (40 HP, 20+20), Mirrex
(`Player.last_minion_played` + трансформація в руці), The Fins Beyond Time
(зберегти стартову руку в `Player` — 10×u16, вміщається), The Egg of Khelos
(ланцюжок токенів через `children()`), Toreth / Elise / Bashana / Irida /
Tiny Pal / Dreambound Raptor — апроксимації з нотатками.

Ціль по завершенні фаз 1–5: **350/350** слотів, усі 12 колод грають кінець
у кінець, `tavernsim matrix` по справжніх мета-колодах замість `curve_deck`.

## 5. Повний список: чого бракує по колодах

(`py` — є в `hs2/impls.py`, `--` — писати з тексту)

**UUB Egg Death Knight (24/30):** Reanimated Pterrordax ×2 [py] «Costs
Corpses instead of Mana» (модель вартості: `legal_actions/apply` мають
дозволити оплату трупами — окремий мікро-механізм), The Egg of Khelos [py],
Staff of the Endbringer [py], Blood Doctor Thal'ena [py], Emergency Surgery [py].

**Quest Demon Hunter (22/30):** Unleash the Colossus [py], Broxigar [py], Axe
of Cenarius [--], First Portal to Argus [--], Nespirah, Enthralled [py], Sigil
of the Seas ×2 [py], Irida Sinseeker [py].

**Raza Demon Hunter (21/30):** Broxigar [py], Axe of Cenarius [--], First
Portal to Argus [--], Eredar Deceptor ×2 [py], Dark Bribe ×2 [py], Soul
Immolation ×2 [py].

**Attack Druid (20/30):** Elise the Navigator [py], Ebb and Flow ×2 [py],
Acceleration Aura ×2 [py], Wickerfang [py], Merithra of the Dream [py],
Bashana Runetotem [py], Spiderling ×2 [py].

**Quest Hunter (20/30):** Shaladrassil [py], Cower in Fear ×2 [py], The Food
Chain [py], Platysaur ×2 [py], Storm the Gates [py], Earthen Roar [py], Guard
Dog ×2 [py].

**Fatigue Paladin (18/30 + sideboard Prize Vendor):** Toreth the Unbreaking
[py], Ursol [py], Hardlight Protector ×2 [py], The Fins Beyond Time [py],
Acceleration Aura ×2 [py], Twilight Egg ×2 [py], Judgment ×2 [py], Commander
Beatrix [py].

**Thief Priest (7/20):** Lunarwing Messenger ×2 [py], Atiesh the Greatstaff
[--], Karazhan the Sanctum [--], Intertwined Fate ×2 [py], Soothsayer ×2 [py],
Azalina Soulsever [py], Mind Sweeper ×2 [py], Unshackle Soul ×2 [py].

**Herald Rogue (10/30):** Naralex, Herald of the Flights [py], Nightmare Fuel
×2 [py], Shaladrassil [py], Cultist Map ×2 [py], Elise the Navigator [py],
Mirrex, the Crystalline [py], Opu the Unseen [py], Garona Halforcen [py], King
Llane [--], The Kingslayers [--], Deja Vu ×2 [py], Ultraxion [py], Agent of the
Old Ones ×2 [py], Sinestra [py], Deathwing, Worldbreaker [py], Shadow of
Demise [py].

**Zee Shaman (20/30):** Cult Neophyte [py], Dreambound Raptor [py], Platysaur
×2 [py], Ultraxion [py], Al'Akir, Lord of Storms [py], Deathwing, Worldbreaker
[py], Mug'Zee [py], Tiny Pal [py], Getaway Hogdriver [py].

**Herald Warlock (16/30):** Rotten Apple [py], Cursed Catacombs ×2 [py],
Cursed Chains ×2 [py], Ultraxion [py], Deathwing, Worldbreaker [py], Imp Gang
Stooge ×2 [py], Godfrey the Betrayer [py], Annihilation ×2 [py], Spire of
Solitude ×2 [py].

**Dragon Warrior (16/30):** Darkrider ×2 [py], Brood Keeper ×2 [py],
Shadowflame Suffusion ×2 [py], Stadium Announcer ×2 [py], Erupting Volcano ×2
[py], Torch ×2 [py], Warptooth [py], Chainbreaker Hogger [py].

Найчастіші серед колод: Deathwing, Worldbreaker (3 колоди), Ultraxion (3),
Acceleration Aura (2 колоди, 4 слоти), Platysaur (2/4), Axe of Cenarius (2),
Broxigar (2), Elise (2), First Portal to Argus (2), Shaladrassil (2).

## 6. Правила, яких дотримуватись (з самого коду, не з голови)

1. Одна карта — один рядок у `BEHAVIOURS` і один тест у `core/tests/cards.rs`,
   що стверджує **стан** після розіграшу. Карта без тесту не вважається
   зробленою.
2. Бракує дієслова — додати в `effects.rs` з doc-коментарем; не читати/писати
   `players[..]` напряму з рядка карти.
3. Жодних алокацій у гарячому шляху: `Inline<T, N>`, `Copy`-поля. Будь-яке нове
   поле в `Player`/`Permanent`/`HandCard` перевіряти тестом `a_game_is_small_enough_to_copy_freely` (`Game` < 2048 байт).
4. Часткова реалізація — тільки **слабша** за оригінал і тільки з записом в
   `APPROXIMATE` (назва + чого бракує). Якщо не виходить слабша — сказати це в
   нотатці явно, як для Archmage Kalec. Ніяких мовчазних заглушок.
5. Токени — через `tokens::` + `token("ID")` або `summonable_children()`;
   ніколи не `by_name("…").unwrap()` у рядку карти.
6. Назва в рядку — точно як у корпусі (тест `every_declared_card_exists_in_the_corpus`
   зловить одруківку; апострофи/коми як у `INFO[..].name`).
7. `table.rs` не редагувати; при зміні даних — повний пайплайн із §2 і окремий
   коміт «regenerate table».
8. Python — для семантики; текст карти в корпусі — істина. Розбіжність між
   ними — записати в коментар до рядка.
9. Не ламати `most_classes_can_field_a_curve_deck` (≥ 8 класів) і «deck: yes»
   для всіх 11 класів у `coverage`.
10. Коміт на транш (10–20 карт або один механізм), у повідомленні — числа
    coverage/gauntlet «було → стало» і кількість тестів.

## 7. Що НЕ входить у цей план

- Деккод-декодер у Rust (мета-колоди читаються з JSON по назвах — достатньо).
- Wild-покриття, Twist, арена.
- Переписування AI (`agent.rs`) — окрема тема; комбо-колоди й далі будуть
  недооцінені скриптовим агентом (див. README «Відомі обмеження»).
- Інтеграція Rust-рушія в `evaluate.py`/`app.py` — після 350/350.
