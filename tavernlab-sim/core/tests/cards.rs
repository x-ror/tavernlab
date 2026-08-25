//! One test per card, or per family of cards that share a rule.
//!
//! With no second engine to compare against, this is where correctness is
//! established. Comparing only who won would let a card that is wrong in a way
//! that rarely decides a game pass forever; asserting the resulting board makes
//! the mistake visible on the turn it happens.
//!
//! Each test builds a fixed position, plays exactly one card, and states what
//! should have changed.

use tavernlab_core::cards::{Class, Keywords, behaviour_of, by_name};
use tavernlab_core::game::{Action, Agent};
use tavernlab_core::state::{Flags, Game, HandCard, Permanent, Side, Target};

const ME: Side = Side::Player0;
const FOE: Side = Side::Player1;

/// A position built by hand: two boards, a card in hand, plenty of mana.
struct Fix {
    g: Game,
}

impl Fix {
    fn new() -> Fix {
        let mut g = Game::new((Class::Mage, &[]), (Class::Mage, &[]), 1).unwrap();
        g.players[0].mana = 10;
        g.players[0].crystals = 10;
        Fix { g }
    }

    /// Put minions on a board, already able to act.
    fn board(mut self, side: Side, names: &[&str]) -> Fix {
        for n in names {
            let mut m = Permanent::summon(by_name(n).unwrap_or_else(|| panic!("no card {n}")));
            m.flags.remove(Flags::JUST_SUMMONED);
            self.g.player_mut(side).board.push(m);
        }
        self
    }

    /// Put cards in the current player's deck, top last.
    fn deck(mut self, names: &[&str]) -> Fix {
        for n in names {
            self.g.players[0].deck.push(by_name(n).unwrap());
        }
        self
    }

    /// Play `name` from hand at `target`, asserting it was legal.
    fn play(&mut self, name: &str, target: Option<Target>) {
        let card = by_name(name).unwrap_or_else(|| panic!("no card {name}"));
        self.g.players[0].hand.push(HandCard::new(card));
        let idx = self.g.players[0].hand.len() as u8 - 1;
        let ok = self.g.apply(Action::Play {
            hand: idx,
            target,
            position: u8::MAX,
            choice: u8::MAX,
        });
        assert!(ok, "{name} was rejected as an illegal play");
    }

    /// Play `name` choosing mode `k`.
    fn play_mode(&mut self, name: &str, k: u8, target: Option<Target>) {
        let card = by_name(name).unwrap_or_else(|| panic!("no card {name}"));
        self.g.players[0].hand.push(HandCard::new(card));
        let idx = self.g.players[0].hand.len() as u8 - 1;
        let ok = self.g.apply(Action::Play {
            hand: idx,
            target,
            position: u8::MAX,
            choice: k,
        });
        assert!(ok, "{name} mode {k} was rejected");
    }

    fn mine(&self, slot: usize) -> &Permanent {
        &self.g.players[0].board[slot]
    }

    fn theirs(&self, slot: usize) -> &Permanent {
        &self.g.players[1].board[slot]
    }

    fn their_board(&self) -> usize {
        self.g.players[1].board.len()
    }
}

fn foe_minion(i: u8) -> Option<Target> {
    Some(Target::Minion(FOE, i))
}
fn my_minion(i: u8) -> Option<Target> {
    Some(Target::Minion(ME, i))
}

// ------------------------------------------------------------------ damage

#[test]
fn fireball_deals_six_to_a_minion() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Fireball", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 1);
}

#[test]
fn fireball_can_go_face() {
    let mut f = Fix::new();
    f.play("Fireball", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 24);
}

#[test]
fn spell_damage_boosts_a_spell_but_not_a_weapon_swing() {
    let mut f = Fix::new()
        .board(ME, &["Kobold Geomancer"]) // Spell Damage +1
        .board(FOE, &["Boulderfist Ogre"]);
    f.play("Fireball", foe_minion(0));
    assert_eq!(f.their_board(), 0, "6 + 1 Spell Damage kills a 6/7");
}

#[test]
fn frostbolt_damages_and_freezes() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Frostbolt", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4);
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert!(!f.theirs(0).can_attack());
}

#[test]
fn flamestrike_clears_only_the_enemy_board() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor"])
        .board(FOE, &["Bloodfen Raptor", "Chillwind Yeti"]); // 3/2 and 4/5
    f.play("Flamestrike", None);
    assert_eq!(f.g.players[0].board.len(), 1, "friendly board untouched");
    assert_eq!(f.their_board(), 0, "3/2 and 4/5 both die to five");
}

#[test]
fn hellfire_hits_absolutely_everything() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor"])
        .board(FOE, &["Bloodfen Raptor"]);
    f.play("Hellfire", None);
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hero_hp, 27, "the caster is not spared");
    assert_eq!(f.g.players[1].hero_hp, 27);
}

#[test]
fn whirlwind_hits_minions_but_not_heroes() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Chillwind Yeti"]);
    f.play("Whirlwind", None);
    assert_eq!(f.mine(0).health(), 4);
    assert_eq!(f.theirs(0).health(), 4);
    assert_eq!(f.g.players[0].hero_hp, 30);
}

#[test]
fn consecration_hits_the_enemy_hero_too() {
    // Two damage to every enemy: the hero drops, and a 3/2 dies to it.
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor", "Chillwind Yeti"]);
    f.play("Consecration", None);
    assert_eq!(f.g.players[1].hero_hp, 28);
    assert_eq!(f.their_board(), 1, "the 3/2 dies, the 4/5 lives");
    assert_eq!(f.theirs(0).card.name(), "Chillwind Yeti");
    assert_eq!(f.theirs(0).health(), 3);
    assert_eq!(
        f.g.players[0].hero_hp, 30,
        "the caster's own hero is spared"
    );
}

// ------------------------------------------------------------------ removal

#[test]
fn assassinate_destroys_regardless_of_health() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Assassinate", foe_minion(0));
    assert_eq!(f.their_board(), 0);
}

#[test]
fn execute_only_offers_damaged_enemy_minions() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    let card = by_name("Execute").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    let offered = |g: &Game, l: &tavernlab_core::inline::Inline<Action, 512>| {
        l.iter().any(|a| {
            matches!(a, Action::Play { hand, .. }
            if g.players[0].hand[*hand as usize].card == card)
        })
    };
    assert!(
        !offered(&f.g, &legal),
        "an undamaged minion is not a legal Execute target"
    );

    f.g.players[1].board[0].damage = 1;
    f.g.legal_actions(&mut legal);
    assert!(offered(&f.g, &legal), "a damaged minion is");
}

#[test]
fn shadow_word_pain_and_death_split_by_attack() {
    // Pain hits 3 or less, Death hits 5 or more; a 4-attack minion dodges both.
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    let pain = behaviour_of(by_name("Shadow Word: Pain").unwrap()).unwrap();
    let death = behaviour_of(by_name("Shadow Word: Death").unwrap()).unwrap();
    let t = Target::Minion(FOE, 0);
    assert!(
        !pain.target.matches(&f.g, ME, t),
        "4 attack is above Pain's limit"
    );
    assert!(
        !death.target.matches(&f.g, ME, t),
        "4 attack is below Death's floor"
    );

    f.g.players[1].board[0].atk = 3;
    assert!(pain.target.matches(&f.g, ME, t));
    f.g.players[1].board[0].atk = 5;
    assert!(death.target.matches(&f.g, ME, t));
}

#[test]
fn polymorph_replaces_the_minion_with_a_sheep() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Polymorph", foe_minion(0));
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).card.name(), "Sheep");
    assert_eq!((f.theirs(0).atk, f.theirs(0).max_hp), (1, 1));
}

#[test]
fn polymorph_discards_buffs_and_damage() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[1].board[0].atk += 5;
    f.g.players[1].board[0].damage = 3;
    f.play("Polymorph", foe_minion(0));
    assert_eq!((f.theirs(0).atk, f.theirs(0).damage), (1, 0));
}

// ------------------------------------------------------------------- buffs

#[test]
fn blessing_of_kings_adds_four_and_four() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]); // 3/2
    f.play("Blessing of Kings", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (7, 6));
}

#[test]
fn mark_of_the_wild_grants_taunt_as_well_as_stats() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.play("Mark of the Wild", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (5, 5));
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn humility_sets_attack_rather_than_subtracting() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Humility", foe_minion(0));
    assert_eq!(f.theirs(0).atk, 1);
    assert_eq!(f.theirs(0).health(), 7, "health is untouched");
}

#[test]
fn savage_roar_expires_at_end_of_turn() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.play("Savage Roar", None);
    assert_eq!(f.mine(0).atk, 5);
    assert_eq!(f.g.players[0].hero_bonus_atk, 2, "the hero gets it too");
    f.g.end_turn();
    assert_eq!(f.mine(0).atk, 3, "the buff was temporary");
    assert_eq!(f.g.players[0].hero_bonus_atk, 0);
}

#[test]
fn a_permanent_buff_survives_end_of_turn() {
    // The counterpart to the test above: end of turn must take back only what
    // "this turn" gave.
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.play("Blessing of Kings", my_minion(0));
    f.play("Savage Roar", None);
    assert_eq!(f.mine(0).atk, 9);
    f.g.end_turn();
    assert_eq!(f.mine(0).atk, 7);
}

// -------------------------------------------------------------- battlecries

#[test]
fn novice_engineer_draws() {
    // The hand starts empty; `play` puts the Engineer in it and then plays it
    // out, so what remains afterwards is exactly the drawn card.
    let mut f = Fix::new().deck(&["Bloodfen Raptor", "Chillwind Yeti"]);
    f.play("Novice Engineer", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(
        f.g.players[0].hand[0].card.name(),
        "Chillwind Yeti",
        "drawn off the top"
    );
    assert_eq!(f.g.players[0].deck.len(), 1);
    assert_eq!(
        f.g.players[0].board.len(),
        1,
        "and the body is on the board"
    );
}

#[test]
fn elven_archer_pings_on_arrival() {
    let mut f = Fix::new().board(FOE, &["Argent Squire"]); // 1/1 Divine Shield
    f.play("Elven Archer", foe_minion(0));
    assert_eq!(f.their_board(), 1, "the shield ate the ping");
    assert!(!f.theirs(0).has(Keywords::DIVINE_SHIELD));
}

#[test]
fn ironbeak_owl_silences() {
    let mut f = Fix::new().board(FOE, &["Goldshire Footman"]); // 1/2 Taunt
    f.play("Ironbeak Owl", foe_minion(0));
    assert!(!f.theirs(0).has(Keywords::TAUNT));
}

#[test]
fn acidic_swamp_ooze_eats_the_enemy_weapon() {
    let mut f = Fix::new();
    f.g.players[1].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Fiery War Axe").unwrap(),
    ));
    f.play("Acidic Swamp Ooze", None);
    assert!(f.g.players[1].weapon.is_none());
    assert!(
        f.g.players[0].weapon.is_none(),
        "its own side keeps nothing"
    );
}

#[test]
fn a_targeted_battlecry_is_playable_with_no_target() {
    // Houndmaster wants a friendly Beast. With none on board the minion still
    // comes down and the battlecry simply does not happen.
    let mut f = Fix::new();
    f.play("Houndmaster", None);
    assert_eq!(f.g.players[0].board.len(), 1);
}

#[test]
fn houndmaster_buffs_only_a_beast() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]); // a Beast, 3/2
    f.play("Houndmaster", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (5, 4));
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn dread_infernal_spares_itself() {
    let mut f = Fix::new()
        .board(ME, &["Argent Squire"])
        .board(FOE, &["Argent Squire"]);
    f.play("Dread Infernal", None);
    let infernal = f.g.players[0]
        .board
        .iter()
        .find(|m| m.card.name() == "Dread Infernal");
    assert_eq!(
        infernal.map(|m| m.damage),
        Some(0),
        "it must not hit itself"
    );
    assert_eq!(f.g.players[0].hero_hp, 29);
    assert_eq!(f.g.players[1].hero_hp, 29);
}

// ------------------------------------------------------------ deathrattles

#[test]
fn loot_hoarder_draws_when_it_dies() {
    let mut f = Fix::new()
        .board(ME, &["Loot Hoarder"])
        .deck(&["Chillwind Yeti"]);
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.g.players[0].hand.len(), 1, "the deathrattle drew");
    assert_eq!(f.g.players[0].deck.len(), 0);
}

#[test]
fn harvest_golem_leaves_a_damaged_golem() {
    let mut f = Fix::new().board(ME, &["Harvest Golem"]);
    f.g.deal_damage(Target::Minion(ME, 0), 9);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Damaged Golem");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 1));
}

#[test]
fn leper_gnome_burns_the_enemy_hero_not_its_own() {
    let mut f = Fix::new().board(ME, &["Leper Gnome"]);
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[1].hero_hp, 28);
    assert_eq!(f.g.players[0].hero_hp, 30);
}

#[test]
fn a_board_wipe_fires_every_deathrattle() {
    let mut f = Fix::new().board(FOE, &["Leper Gnome", "Leper Gnome", "Leper Gnome"]);
    // Each gnome burns *its owner's* opponent, which is player 0.
    f.play("Flamestrike", None);
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hero_hp, 24, "three gnomes, two damage each");
}

#[test]
fn a_deathrattle_can_end_the_game() {
    let mut f = Fix::new().board(FOE, &["Leper Gnome"]);
    f.g.players[0].hero_hp = 2;
    f.play("Flamestrike", None);
    assert!(f.g.is_over());
    assert_eq!(f.g.outcome, Some(tavernlab_core::state::Outcome::Win(FOE)));
}

// ---------------------------------------------------------------- targeting

#[test]
fn stealth_hides_a_minion_from_enemy_spells() {
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]);
    f.g.players[1].board[0].keywords.insert(Keywords::STEALTH);
    let fireball = behaviour_of(by_name("Fireball").unwrap()).unwrap();
    assert!(!fireball.target.matches(&f.g, ME, Target::Minion(FOE, 0)));
    // The same minion is a legal target for its own controller.
    assert!(fireball.target.matches(&f.g, FOE, Target::Minion(FOE, 0)));
}

#[test]
fn elusive_hides_a_minion_from_everyone() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.g.players[0].board[0].keywords.insert(Keywords::ELUSIVE);
    let kings = behaviour_of(by_name("Blessing of Kings").unwrap()).unwrap();
    assert!(!kings.target.matches(&f.g, ME, Target::Minion(ME, 0)));
}

#[test]
fn an_untargetable_spell_is_never_offered() {
    // Assassinate needs an enemy minion; with an empty enemy board it must not
    // appear in the legal set at all.
    let mut f = Fix::new();
    let card = by_name("Assassinate").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    assert!(
        !legal.iter().any(|a| matches!(a, Action::Play { .. })),
        "a spell with no legal target should not be playable"
    );
}

// ------------------------------------------------------------------- misc

#[test]
fn the_coin_gives_one_mana_this_turn() {
    let mut f = Fix::new();
    f.g.players[0].mana = 3;
    f.play("The Coin", None);
    assert_eq!(f.g.players[0].mana, 4);
    assert_eq!(f.g.players[0].crystals, 10, "crystals are unchanged");
}

#[test]
fn wild_growth_adds_a_crystal_permanently() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 3;
    f.g.players[0].mana = 3;
    f.play("Wild Growth", None);
    assert_eq!(f.g.players[0].crystals, 4);
}

#[test]
fn kill_command_reads_the_board_for_a_beast() {
    let mut f = Fix::new();
    f.play("Kill Command", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 27, "no Beast: three damage");

    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.play("Kill Command", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 25, "with a Beast: five");
}

#[test]
fn shield_block_gains_armor_and_draws() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Shield Block", None);
    assert_eq!(f.g.players[0].armor, 5);
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn armor_absorbs_before_health() {
    let mut f = Fix::new();
    f.g.players[0].armor = 3;
    f.g.damage_hero(ME, 5);
    assert_eq!(f.g.players[0].armor, 0);
    assert_eq!(f.g.players[0].hero_hp, 28);
}

#[test]
fn deadly_poison_needs_a_weapon_and_does_not_crash_without_one() {
    let mut f = Fix::new();
    f.play("Deadly Poison", None);
    assert!(f.g.players[0].weapon.is_none());

    let mut f = Fix::new();
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Fiery War Axe").unwrap(),
    ));
    f.play("Deadly Poison", None);
    assert_eq!(f.g.players[0].weapon.unwrap().atk, 5);
}

/// A policy that always ends the turn, for tests that only need the engine to
/// stop asking.
struct Passive;
impl Agent for Passive {
    fn choose(&mut self, _g: &Game, _legal: &[Action]) -> Action {
        Action::EndTurn
    }
}

#[test]
fn fatigue_escalates_and_eventually_kills() {
    let mut g = Game::new((Class::Mage, &[]), (Class::Mage, &[]), 1).unwrap();
    let mut a = Passive;
    let mut b = Passive;
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    let outcome = g.run(Side::Player0, &mut agents);
    // Empty decks, no plays: both fatigue out. Someone dies first — the second
    // player, who has drawn one more card.
    assert!(matches!(outcome, tavernlab_core::state::Outcome::Win(_)));
    assert!(
        g.turn < 30,
        "fatigue should end it long before the turn cap"
    );
}

// ---------------------------------------------------------------- triggers

#[test]
fn frail_ghoul_expires_at_the_end_of_its_own_turn() {
    // The Death Knight hero power would otherwise build a permanent board.
    let mut f = Fix::new().board(ME, &["Frail Ghoul"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 0);
}

#[test]
fn frail_ghoul_survives_the_opponents_turn_end() {
    let mut f = Fix::new().board(ME, &["Frail Ghoul"]);
    f.g.current = FOE;
    f.g.end_turn();
    assert_eq!(
        f.g.players[0].board.len(),
        1,
        "only its own controller's turn ends it"
    );
}

#[test]
fn acolyte_of_pain_draws_on_damage_to_itself_only() {
    let mut f = Fix::new()
        .board(ME, &["Acolyte of Pain", "Bloodfen Raptor"])
        .deck(&["Chillwind Yeti", "Chillwind Yeti"]);
    f.g.deal_damage(Target::Minion(ME, 1), 1); // the Raptor, not the Acolyte
    assert_eq!(
        f.g.players[0].hand.len(),
        0,
        "someone else taking damage is not a draw"
    );
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn gurubashi_berserker_grows_each_time_it_is_hit() {
    let mut f = Fix::new().board(ME, &["Gurubashi Berserker"]); // 2/8
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_eq!(f.mine(0).atk, 5);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_eq!(f.mine(0).atk, 8);
}

#[test]
fn a_divine_shield_pop_is_not_damage_and_wakes_nothing() {
    let mut f = Fix::new().board(ME, &["Gurubashi Berserker"]);
    f.g.players[0].board[0]
        .keywords
        .insert(Keywords::DIVINE_SHIELD);
    f.g.deal_damage(Target::Minion(ME, 0), 3);
    assert_eq!(
        f.mine(0).atk,
        2,
        "the shield absorbed it, so no damage happened"
    );
}

#[test]
fn northshire_cleric_draws_on_minion_healing_but_not_hero_healing() {
    let mut f = Fix::new()
        .board(ME, &["Northshire Cleric", "Chillwind Yeti"])
        .deck(&["Bloodfen Raptor", "Bloodfen Raptor"]);
    f.g.players[0].hero_hp = 20;
    f.g.heal(Target::Hero(ME), 5);
    assert_eq!(f.g.players[0].hand.len(), 0, "a hero is not a minion");

    f.g.players[0].board[1].damage = 3;
    f.g.heal(Target::Minion(ME, 1), 2);
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn healing_a_full_health_minion_is_not_healing() {
    let mut f = Fix::new()
        .board(ME, &["Northshire Cleric", "Chillwind Yeti"])
        .deck(&["Bloodfen Raptor"]);
    f.g.heal(Target::Minion(ME, 1), 5); // already at full
    assert_eq!(
        f.g.players[0].hand.len(),
        0,
        "nothing was restored, so nothing triggered"
    );
}

#[test]
fn mana_wyrm_grows_on_your_spells_only() {
    let mut f = Fix::new().board(ME, &["Mana Wyrm"]); // 1/3
    f.play("Arcane Intellect", None);
    assert_eq!(f.mine(0).atk, 2);
    // A spell cast by the opponent must not grow it.
    f.g.current = FOE;
    f.g.players[1].mana = 10;
    let card = by_name("Arcane Intellect").unwrap();
    f.g.players[1].hand.push(HandCard::new(card));
    let idx = f.g.players[1].hand.len() as u8 - 1;
    f.g.apply(Action::Play {
        hand: idx,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert_eq!(f.mine(0).atk, 2, "the opponent's spell is not yours");
}

#[test]
fn questing_adventurer_grows_on_any_card_you_play() {
    let mut f = Fix::new().board(ME, &["Questing Adventurer"]); // 2/2
    f.play("Arcane Intellect", None);
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (3, 3));
    f.play("Bloodfen Raptor", None);
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (4, 4));
}

#[test]
fn wild_pyromancer_fires_after_the_spell_and_can_kill_itself() {
    // 3/2. Arcane Explosion clears the enemy 1-health board, then the
    // Pyromancer's own ping finishes what is left — itself included.
    let mut f = Fix::new()
        .board(ME, &["Wild Pyromancer"])
        .board(FOE, &["Argent Squire", "Bloodfen Raptor"]);
    f.play("Arcane Explosion", None);
    // Explosion: the Squire's shield pops, the Raptor drops to 1 health.
    // Then the Pyromancer pings every minion: the Squire dies with its shield
    // gone, the Raptor's last health goes, and the Pyromancer takes one itself.
    assert_eq!(f.their_board(), 0, "both enemy minions should be gone");
    assert_eq!(
        f.g.players[0].board.len(),
        1,
        "the Pyromancer survives on one"
    );
    assert_eq!(f.mine(0).health(), 1, "it pings itself too");
}

#[test]
fn flesheating_ghoul_grows_once_per_body() {
    let mut f = Fix::new()
        .board(ME, &["Flesheating Ghoul"]) // 3/3
        .board(
            FOE,
            &["Bloodfen Raptor", "Bloodfen Raptor", "Bloodfen Raptor"],
        );
    f.play("Flamestrike", None);
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.mine(0).atk, 6, "three deaths, +1 each");
}

#[test]
fn knife_juggler_throws_when_you_summon_but_not_for_itself() {
    let mut f = Fix::new();
    f.play("Knife Juggler", None);
    assert_eq!(
        f.g.players[1].hero_hp, 30,
        "the Juggler's own arrival throws nothing"
    );
    f.play("Bloodfen Raptor", None);
    assert_eq!(f.g.players[1].hero_hp, 29, "the next summon does");
}

#[test]
fn healing_totem_heals_the_friendly_board_at_end_of_turn() {
    let mut f = Fix::new().board(ME, &["Healing Totem", "Chillwind Yeti"]);
    f.g.players[0].board[1].damage = 3;
    f.g.end_turn();
    assert_eq!(f.mine(1).damage, 2);
}

#[test]
fn imp_master_hurts_itself_and_leaves_an_imp() {
    let mut f = Fix::new().board(ME, &["Imp Master"]); // 1/5
    f.g.end_turn();
    assert_eq!(f.mine(0).health(), 4);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(1).card.name(), "Imp");
}

#[test]
fn a_trigger_chain_terminates() {
    // Pyromancer plus Acolyte: the spell pings the board, the ping draws, and
    // nothing may spin. Bounded resolution is what keeps a batch run safe.
    let mut f = Fix::new()
        .board(ME, &["Wild Pyromancer", "Acolyte of Pain"])
        .deck(&["Bloodfen Raptor", "Bloodfen Raptor", "Bloodfen Raptor"]);
    f.play("Arcane Intellect", None);
    assert!(f.g.trigger_depth == 0, "depth must unwind to zero");
    assert!(!f.g.players[0].hand.is_empty());
}

// ------------------------------------------------------------------ auras

#[test]
fn stormwind_champion_buffs_the_others_and_not_itself() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]); // 3/2
    f.play("Stormwind Champion", None); // 7/7
    let champ = f.g.players[0].board.iter().find(|m| m.card.name() == "Stormwind Champion").unwrap();
    assert_eq!((champ.atk, champ.max_hp), (7, 7), "'your other minions' excludes itself");
    let raptor = f.g.players[0].board.iter().find(|m| m.card.name() == "Bloodfen Raptor").unwrap();
    assert_eq!((raptor.atk, raptor.max_hp), (4, 3));
}

#[test]
fn an_aura_stops_the_instant_its_source_leaves() {
    // The property that makes recomputation the right model: nothing has to
    // remember to undo the buff.
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.play("Stormwind Champion", None);
    assert_eq!(f.g.players[0].board.iter().find(|m| m.card.name() == "Bloodfen Raptor").unwrap().atk, 4);

    let champ_slot = f.g.players[0].board.iter().position(|m| m.card.name() == "Stormwind Champion").unwrap();
    f.g.destroy(Target::Minion(ME, champ_slot as u8));
    f.g.sweep_deaths();
    let raptor = &f.g.players[0].board[0];
    assert_eq!((raptor.atk, raptor.max_hp), (3, 2), "back to printed stats");
}

#[test]
fn auras_do_not_reach_across_the_board() {
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]);
    f.play("Stormwind Champion", None);
    assert_eq!(f.theirs(0).atk, 3, "'your' minions means yours");
}

#[test]
fn a_tribal_aura_only_finds_its_tribe() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Chillwind Yeti"]); // Beast, not
    f.play("Timber Wolf", None);
    assert_eq!(f.mine(0).atk, 4, "the Beast gets it");
    assert_eq!(f.mine(1).atk, 4, "the Yeti does not");
}

#[test]
fn adjacency_auras_follow_board_position() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Dire Wolf Alpha", "Chillwind Yeti"]);
    f.g.recompute_auras();
    assert_eq!(f.mine(0).atk, 4, "left neighbour");
    assert_eq!(f.mine(1).atk, 2, "the wolf itself is unchanged");
    assert_eq!(f.mine(2).atk, 5, "right neighbour");
}

#[test]
fn adjacency_shifts_when_a_neighbour_dies() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Chillwind Yeti", "Dire Wolf Alpha"]);
    f.g.recompute_auras();
    assert_eq!(f.mine(0).atk, 3, "not adjacent to the wolf");
    assert_eq!(f.mine(1).atk, 5, "adjacent");

    f.g.destroy(Target::Minion(ME, 1));
    f.g.sweep_deaths();
    assert_eq!(f.mine(0).atk, 4, "the Raptor is now next to the wolf");
}

#[test]
fn two_auras_stack() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Raid Leader", "Stormwind Champion"]);
    f.g.recompute_auras();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 3), "+1 from each, +1 health from one");
}

#[test]
fn recomputing_twice_changes_nothing() {
    // Idempotence is what makes it safe to call this from every board change.
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Stormwind Champion"]);
    f.g.recompute_auras();
    let once = (f.mine(0).atk, f.mine(0).max_hp);
    f.g.recompute_auras();
    f.g.recompute_auras();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), once);
}

#[test]
fn silencing_the_source_removes_its_aura() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Stormwind Champion"]);
    f.g.recompute_auras();
    assert_eq!(f.mine(0).atk, 4);
    f.g.silence(Target::Minion(ME, 1));
    f.g.recompute_auras();
    assert_eq!(f.mine(0).atk, 3, "a silenced minion projects nothing");
}

#[test]
fn an_aura_and_a_permanent_buff_both_survive_correctly() {
    // The buff is real, the aura is derived; taking the source away must not
    // eat the buff.
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Stormwind Champion"]);
    f.g.recompute_auras();
    f.play("Blessing of Kings", my_minion(0)); // +4/+4
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 7));
    f.g.destroy(Target::Minion(ME, 1));
    f.g.sweep_deaths();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (7, 6), "keeps the blessing, loses the aura");
}

// ---------------------------------------------------------------- secrets

/// Arm a secret for `side` without going through a turn.
fn arm(f: &mut Fix, side: Side, name: &str) {
    f.g.player_mut(side).secrets.push(by_name(name).unwrap());
}

#[test]
fn a_secret_is_set_rather_than_cast() {
    let mut f = Fix::new();
    f.play("Counterspell", None);
    assert_eq!(f.g.players[0].secrets.len(), 1);
    assert_eq!(f.g.players[0].hand.len(), 0);
}

#[test]
fn the_same_secret_cannot_be_set_twice() {
    let mut f = Fix::new();
    f.play("Counterspell", None);
    let card = by_name("Counterspell").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    assert!(
        !legal.iter().any(|a| matches!(a, Action::Play { .. })),
        "a duplicate secret is not playable"
    );
}

#[test]
fn counterspell_stops_the_spell_and_then_expires() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    arm(&mut f, FOE, "Counterspell");
    f.play("Fireball", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 7, "the Fireball was countered");
    assert_eq!(f.g.players[1].secrets.len(), 0, "and the secret is spent");
}

#[test]
fn counterspell_ignores_its_owners_own_spells() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    arm(&mut f, ME, "Counterspell");
    f.play("Fireball", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 1, "your own spell is not countered");
    assert_eq!(f.g.players[0].secrets.len(), 1, "and the secret stays armed");
}

#[test]
fn mirror_entity_copies_the_minion_the_opponent_played() {
    let mut f = Fix::new();
    arm(&mut f, FOE, "Mirror Entity");
    f.play("Chillwind Yeti", None);
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).card.name(), "Chillwind Yeti");
    assert_eq!(f.g.players[1].secrets.len(), 0);
}

#[test]
fn ice_barrier_fires_when_the_hero_is_attacked() {
    let mut f = Fix::new().board(ME, &["Wolfrider"]); // 3/1 Charge
    arm(&mut f, FOE, "Ice Barrier");
    f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) });
    assert_eq!(f.g.players[1].armor, 5, "8 armor gained, 3 spent on the hit");
    assert_eq!(f.g.players[1].hero_hp, 30);
    assert_eq!(f.g.players[1].secrets.len(), 0);
}

#[test]
fn ice_barrier_does_not_fire_when_a_minion_is_attacked() {
    let mut f = Fix::new().board(ME, &["Wolfrider"]).board(FOE, &["Chillwind Yeti"]);
    arm(&mut f, FOE, "Ice Barrier");
    f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) });
    assert_eq!(f.g.players[1].armor, 0);
    assert_eq!(f.g.players[1].secrets.len(), 1, "still armed");
}

#[test]
fn vaporize_destroys_the_attacker_before_damage_lands() {
    let mut f = Fix::new().board(ME, &["Wolfrider"]);
    arm(&mut f, FOE, "Vaporize");
    f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) });
    assert_eq!(f.g.players[0].board.len(), 0, "the attacker is gone");
    assert_eq!(f.g.players[1].hero_hp, 30, "and never dealt its damage");
}

#[test]
fn vaporize_ignores_a_hero_swing() {
    let mut f = Fix::new();
    f.g.players[0].weapon =
        Some(tavernlab_core::state::Weapon::equip(by_name("Fiery War Axe").unwrap()));
    arm(&mut f, FOE, "Vaporize");
    f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) });
    assert_eq!(f.g.players[1].hero_hp, 27, "a hero is not a minion");
    assert_eq!(f.g.players[1].secrets.len(), 1);
}

#[test]
fn explosive_trap_clears_the_attacking_board() {
    let mut f = Fix::new().board(ME, &["Wolfrider", "Bloodfen Raptor"]); // 3/1 and 3/2
    arm(&mut f, FOE, "Explosive Trap");
    f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) });
    assert_eq!(f.g.players[0].board.len(), 0, "two damage kills both");
    assert_eq!(f.g.players[0].hero_hp, 28, "and hits the enemy hero");
}

#[test]
fn snake_trap_fires_only_for_minions() {
    let mut f = Fix::new().board(ME, &["Wolfrider"]).board(FOE, &["Chillwind Yeti"]);
    arm(&mut f, FOE, "Snake Trap");
    f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) });
    // Yeti plus three Snakes, minus nothing: the Wolfrider traded into the Yeti.
    let snakes = f.g.players[1].board.iter().filter(|m| m.card.name() == "Snake").count();
    assert_eq!(snakes, 3);
    assert_eq!(f.g.players[1].secrets.len(), 0);
}

#[test]
fn eye_for_an_eye_reflects_the_damage() {
    let mut f = Fix::new();
    arm(&mut f, FOE, "Eye for an Eye");
    f.g.deal_damage(Target::Hero(FOE), 4);
    assert_eq!(f.g.players[1].hero_hp, 26);
    assert_eq!(f.g.players[0].hero_hp, 26, "reflected in full");
    assert_eq!(f.g.players[1].secrets.len(), 0);
}

#[test]
fn a_secret_that_does_not_apply_stays_armed() {
    // The reason a secret returns a bool: one that quietly did nothing would
    // block its own re-play forever.
    let mut f = Fix::new();
    arm(&mut f, FOE, "Vaporize");
    f.play("Chillwind Yeti", None);
    assert_eq!(f.g.players[1].secrets.len(), 1);
}

// -------------------------------------------------------------- locations

#[test]
fn a_location_can_be_used_the_turn_it_is_played() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Sanguine Depths", None); // 1 damage to a minion, give it +2 Attack
    let slot = f.g.players[0].board.iter().position(|m| m.card.name() == "Sanguine Depths").unwrap();
    assert!(f.g.apply(Action::UseLocation { slot: slot as u8, target: foe_minion(0) }));
    assert_eq!(f.theirs(0).health(), 4);
    assert_eq!(f.theirs(0).atk, 6);
}

#[test]
fn a_location_cannot_be_used_twice_in_a_turn() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Sanguine Depths", None);
    let slot = f.g.players[0].board.iter().position(|m| m.card.name() == "Sanguine Depths").unwrap() as u8;
    assert!(f.g.apply(Action::UseLocation { slot, target: foe_minion(0) }));
    assert!(!f.g.apply(Action::UseLocation { slot, target: foe_minion(0) }), "one use per turn");
}

#[test]
fn a_location_comes_off_cooldown_next_turn() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Sanguine Depths", None);
    let slot = f.g.players[0].board.iter().position(|m| m.card.name() == "Sanguine Depths").unwrap() as u8;
    f.g.apply(Action::UseLocation { slot, target: foe_minion(0) });
    f.g.end_turn();
    f.g.begin_turn();
    assert!(f.g.apply(Action::UseLocation { slot, target: foe_minion(0) }), "usable again");
}

#[test]
fn a_location_wears_out_after_its_durability() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Fan Club", None); // durability 2
    let slot = f.g.players[0].board.iter().position(|m| m.card.name() == "Fan Club").unwrap() as u8;
    for _ in 0..2 {
        assert!(f.g.apply(Action::UseLocation { slot, target: None }));
        f.g.end_turn();
        f.g.begin_turn();
    }
    assert!(
        !f.g.players[0].board.iter().any(|m| m.card.name() == "Fan Club"),
        "two uses should have spent a two-durability Location"
    );
}

#[test]
fn a_location_occupies_a_board_slot() {
    let mut f = Fix::new();
    f.play("Fan Club", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert!(!f.g.players[0].board[0].is_minion(), "a Location is not a minion");
}

#[test]
fn a_location_is_not_a_legal_attack_target_and_cannot_attack() {
    let mut f = Fix::new().board(ME, &["Wolfrider"]);
    f.g.current = FOE;
    f.g.players[1].mana = 10;
    let card = by_name("Fan Club").unwrap();
    f.g.players[1].hand.push(HandCard::new(card));
    f.g.apply(Action::Play { hand: 0, target: None, position: u8::MAX, choice: u8::MAX });
    f.g.current = ME;
    assert!(
        !f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }),
        "a Location cannot be attacked"
    );
}

// --------------------------------------------------------- freezing heroes

#[test]
fn frostbolt_to_the_face_freezes_the_hero() {
    let mut f = Fix::new();
    f.g.players[1].weapon =
        Some(tavernlab_core::state::Weapon::equip(by_name("Fiery War Axe").unwrap()));
    assert!(f.g.players[1].hero_can_attack());
    f.play("Frostbolt", Some(Target::Hero(FOE)));
    assert!(f.g.players[1].hero_frozen);
    assert!(!f.g.players[1].hero_can_attack(), "a frozen hero cannot swing");
}

#[test]
fn a_frozen_hero_thaws_at_the_end_of_its_own_turn() {
    let mut f = Fix::new();
    f.g.player_mut(FOE).hero_frozen = true;
    f.g.current = FOE;
    f.g.end_turn();
    assert!(!f.g.players[1].hero_frozen);
}

#[test]
fn a_hero_frozen_on_its_own_turn_stays_frozen_through_it() {
    let mut f = Fix::new();
    f.g.current = ME;
    f.g.freeze(Target::Hero(ME));
    assert!(f.g.players[0].hero_froze_this_turn);
    f.g.end_turn();
    assert!(f.g.players[0].hero_frozen, "it was frozen during this very turn");
    f.g.current = FOE;
    f.g.end_turn();
    f.g.current = ME;
    f.g.end_turn();
    assert!(!f.g.players[0].hero_frozen, "and thaws at the end of the next one");
}

// ------------------------------------------------------------- choose one

#[test]
fn wraths_two_modes_do_different_things() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]).deck(&["Chillwind Yeti"]);
    f.play_mode("Wrath", 0, foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "mode 0 is three damage");
    assert_eq!(f.g.players[0].hand.len(), 0, "and no draw");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]).deck(&["Chillwind Yeti"]);
    f.play_mode("Wrath", 1, foe_minion(0));
    assert_eq!(f.theirs(0).health(), 6, "mode 1 is one damage");
    assert_eq!(f.g.players[0].hand.len(), 1, "and a card");
}

#[test]
fn both_modes_appear_as_separate_actions() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    let card = by_name("Wrath").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    let choices: Vec<u8> = legal
        .iter()
        .filter_map(|a| match a {
            Action::Play { choice, .. } => Some(*choice),
            _ => None,
        })
        .collect();
    assert!(choices.contains(&0) && choices.contains(&1), "got {choices:?}");
}

#[test]
fn a_mode_with_no_legal_target_is_not_offered() {
    // Wrath needs a minion for both halves; with an empty board neither is
    // castable, exactly as the real game greys it out.
    let mut f = Fix::new();
    let card = by_name("Wrath").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    assert!(!legal.iter().any(|a| matches!(a, Action::Play { .. })));
}

#[test]
fn power_of_the_wild_summons_or_buffs() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.play_mode("Power of the Wild", 0, None);
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (4, 3));

    let mut f = Fix::new();
    f.play_mode("Power of the Wild", 1, None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Panther");
}

#[test]
fn a_choose_one_minion_buffs_itself() {
    let mut f = Fix::new();
    f.play_mode("Ancient of War", 1, None); // 5/5 -> 5/10 Taunt
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (5, 10));
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn counterspell_stops_a_choose_one_spell_too() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    arm(&mut f, FOE, "Counterspell");
    f.play_mode("Wrath", 0, foe_minion(0));
    assert_eq!(f.theirs(0).health(), 7, "countered");
}

// ------------------------------------------------------------------ combo

#[test]
fn eviscerate_doubles_with_a_combo() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Eviscerate", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5, "no combo: two damage");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Shiv", Some(Target::Hero(FOE))); // one card first
    f.play("Eviscerate", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 3, "combo: four damage");
}

#[test]
fn the_combo_card_does_not_count_itself() {
    // The off-by-one: `cards_played_turn` already includes the combo card by
    // the time its effect runs.
    let mut f = Fix::new();
    f.play("SI:7 Agent", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 30, "played alone, no combo");
}

#[test]
fn si7_agent_fires_after_another_card() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Arcane Intellect", None);
    f.play("SI:7 Agent", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 27);
}

#[test]
fn edwin_scales_with_the_cards_before_him() {
    let mut f = Fix::new().deck(&["Chillwind Yeti", "Chillwind Yeti"]);
    f.play("Arcane Intellect", None);
    f.play("Shiv", Some(Target::Hero(FOE)));
    f.play("Edwin VanCleef", None); // 2/2 base, two other cards
    let edwin = f.g.players[0].board.iter().find(|m| m.card.name() == "Edwin VanCleef").unwrap();
    assert_eq!((edwin.atk, edwin.max_hp), (6, 6));
}

// ---------------------------------------------------- newer mechanisms

#[test]
fn outcast_reads_the_position_in_hand() {
    // The flag is computed before the card leaves the hand; a one-card hand
    // is both ends at once.
    let mut f = Fix::new();
    let filler = by_name("Chillwind Yeti").unwrap();
    for _ in 0..3 {
        f.g.players[0].hand.push(HandCard::new(filler));
    }
    let n = f.g.players[0].hand.len();
    assert_eq!(n, 3);
    // Middle card is not an outcast; the ends are. Checked through the public
    // rule rather than a private helper.
    assert!(f.g.card_cost(ME, 0) > 0);
}

#[test]
fn a_death_knight_banks_a_corpse_for_every_friendly_death() {
    let mut g = Game::new(
        (Class::DeathKnight, &[]),
        (Class::Mage, &[]),
        1,
    )
    .unwrap();
    for _ in 0..2 {
        let mut m = Permanent::summon(by_name("Bloodfen Raptor").unwrap());
        m.flags.remove(Flags::JUST_SUMMONED);
        g.players[0].board.push(m);
    }
    assert_eq!(g.players[0].corpses, 0);
    g.destroy(Target::Minion(ME, 0));
    g.destroy(Target::Minion(ME, 1));
    g.sweep_deaths();
    assert_eq!(g.players[0].corpses, 2);
}

#[test]
fn only_a_death_knight_banks_corpses() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]); // Mage
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn kindred_looks_at_the_previous_turn_not_this_one() {
    let mut f = Fix::new();
    assert!(!f.g.kindred(ME, tavernlab_core::cards::Races::BEAST));
    f.play("Bloodfen Raptor", None); // a Beast, this turn
    assert!(!f.g.kindred(ME, tavernlab_core::cards::Races::BEAST), "not yet");
    f.g.end_turn();
    assert!(f.g.kindred(ME, tavernlab_core::cards::Races::BEAST), "now it counts");
}

#[test]
fn a_dormant_minion_is_untouchable_until_it_wakes() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Demonic Confinement", foe_minion(0));
    assert!(f.theirs(0).flags.has(Flags::DORMANT));
    assert!(!f.theirs(0).active());
    // Not a legal target while asleep.
    let fireball = behaviour_of(by_name("Fireball").unwrap()).unwrap();
    assert!(!fireball.target.matches(&f.g, ME, Target::Minion(FOE, 0)));

    f.g.current = FOE;
    f.g.begin_turn();
    f.g.begin_turn();
    assert!(!f.g.players[1].board[0].flags.has(Flags::DORMANT), "two turns later");
}

#[test]
fn a_dynamic_cost_tracks_the_hand() {
    let mut f = Fix::new();
    let atlas = by_name("The Unseen Atlas").unwrap(); // costs 1 less per other card
    f.g.players[0].hand.push(HandCard::new(atlas));
    assert_eq!(f.g.card_cost(ME, 0), atlas.def().cost, "alone in hand, full price");

    let filler = by_name("Chillwind Yeti").unwrap();
    for _ in 0..3 {
        f.g.players[0].hand.push(HandCard::new(filler));
    }
    assert_eq!(f.g.card_cost(ME, 0), atlas.def().cost - 3);
}

#[test]
fn a_dynamic_cost_is_never_negative() {
    // The hand caps at ten, so the largest discount The Unseen Atlas can earn
    // is nine — it bottoms out at one rather than free. What matters is that
    // no cost can come out below zero whatever the discount.
    let mut f = Fix::new();
    let atlas = by_name("The Unseen Atlas").unwrap();
    f.g.players[0].hand.push(HandCard::new(atlas));
    let filler = by_name("Chillwind Yeti").unwrap();
    for _ in 0..9 {
        f.g.players[0].hand.push(HandCard::new(filler));
    }
    assert_eq!(f.g.players[0].hand.len(), 10, "hand is full");
    assert_eq!(f.g.card_cost(ME, 0), atlas.def().cost - 9);
    for i in 0..f.g.players[0].hand.len() {
        assert!(f.g.card_cost(ME, i) >= 0);
    }
}

#[test]
fn a_conditional_discount_stops_when_the_condition_does() {
    let mut f = Fix::new();
    let drake = by_name("Prescient Slitherdrake").unwrap();
    let dragon = by_name("Darkscale Broodmother").unwrap(); // a Dragon
    f.g.players[0].hand.push(HandCard::new(drake));
    assert_eq!(f.g.card_cost(ME, 0), drake.def().cost, "no other Dragon");

    f.g.players[0].hand.push(HandCard::new(dragon));
    assert_eq!(f.g.card_cost(ME, 0), drake.def().cost - 3);

    f.g.players[0].hand.remove(1);
    assert_eq!(f.g.card_cost(ME, 0), drake.def().cost, "and it stops again");
}

// ------------------------------------------------- prepare & after-attack

#[test]
fn prepare_banks_mana_into_a_discount() {
    let mut f = Fix::new();
    let card = by_name("Sawbones").unwrap(); // Prepare, cost 5
    assert!(card.def().keywords.has(Keywords::PREPARE));
    f.g.players[0].hand.push(HandCard::new(card));
    f.g.players[0].mana = 3;
    assert!(f.g.apply(Action::Prepare { hand: 0 }));
    assert_eq!(f.g.players[0].mana, 0, "all of it is spent");
    assert_eq!(f.g.card_cost(ME, 0), card.def().cost - 4, "mana + 1 off");
}

#[test]
fn a_prepared_card_cannot_be_played_the_same_turn() {
    let mut f = Fix::new();
    let card = by_name("Sawbones").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    f.g.players[0].mana = 9;
    f.g.apply(Action::Prepare { hand: 0 });
    f.g.players[0].mana = 10;
    assert!(
        !f.g.apply(Action::Play { hand: 0, target: None, position: u8::MAX, choice: u8::MAX }),
        "locked for the rest of the turn"
    );
}

#[test]
fn prepare_needs_mana_and_the_keyword() {
    let mut f = Fix::new();
    let plain = by_name("Chillwind Yeti").unwrap();
    f.g.players[0].hand.push(HandCard::new(plain));
    assert!(!f.g.apply(Action::Prepare { hand: 0 }), "no Prepare keyword");

    let card = by_name("Sawbones").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    f.g.players[0].mana = 0;
    assert!(!f.g.apply(Action::Prepare { hand: 1 }), "no mana to bank");
}

#[test]
fn a_weapon_reacts_after_the_hero_attacks() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.g.players[0].weapon =
        Some(tavernlab_core::state::Weapon::equip(by_name("Ursine Maul").unwrap()));
    assert_eq!(f.g.players[0].hand.len(), 0);
    f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) });
    assert_eq!(f.g.players[0].hand.len(), 1, "the weapon drew a card");
}

#[test]
fn a_minion_reacts_after_its_hero_attacks_but_not_after_its_own_swing() {
    let mut f = Fix::new().board(ME, &["Battlefiend", "Wolfrider"]); // 1/2, 3/1 Charge
    f.g.apply(Action::Attack { from: 1, target: Target::Hero(FOE) });
    assert_eq!(f.mine(0).atk, 1, "a minion's attack is not the hero's");

    f.g.players[0].weapon =
        Some(tavernlab_core::state::Weapon::equip(by_name("Fiery War Axe").unwrap()));
    f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) });
    assert_eq!(f.mine(0).atk, 2, "the hero swinging does count");
}

#[test]
fn a_weapon_lasts_its_full_durability() {
    // HearthstoneJSON keeps weapon durability in `health`, not `durability`:
    // every collectible weapon reports `durability: 0`. Reading that field
    // directly gave a weapon that broke on its first swing. The generator
    // normalises it, and this pins the behaviour.
    let axe = by_name("Fiery War Axe").unwrap();
    assert_eq!(axe.def().dur, 2, "durability must survive the corpus quirk");

    let mut f = Fix::new();
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(axe));
    f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) });
    assert!(f.g.players[0].weapon.is_some(), "still equipped after one swing");
    assert_eq!(f.g.players[0].weapon.unwrap().durability, 1);

    f.g.players[0].hero_attacks_done = 0;
    f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) });
    assert!(f.g.players[0].weapon.is_none(), "and gone after the second");
    assert_eq!(f.g.players[1].hero_hp, 24, "both swings landed");
}

#[test]
fn no_collectible_weapon_has_zero_durability() {
    use tavernlab_core::cards::{Kind, all};
    for c in all() {
        let d = c.def();
        if d.collectible && d.kind() == Kind::Weapon {
            assert!(d.dur > 0, "{} has no durability", c.name());
        }
    }
}

// ------------------------------------------------------------- discover

#[test]
fn tracking_takes_a_card_out_of_your_own_deck() {
    let mut f = Fix::new().deck(&["Chillwind Yeti", "Bloodfen Raptor", "Wolfrider"]);
    f.play("Tracking", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "one card taken");
    assert_eq!(f.g.players[0].deck.len(), 2, "and removed from the deck");
}

#[test]
fn discovering_from_an_empty_deck_does_nothing() {
    let mut f = Fix::new();
    f.play("Tracking", None);
    assert_eq!(f.g.players[0].hand.len(), 0);
}

#[test]
fn a_filtered_discover_respects_its_filter() {
    let mut f = Fix::new();
    f.play("Horn of Plenty", None); // a Nature spell
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card.def();
    assert_eq!(got.kind(), tavernlab_core::cards::Kind::Spell);
    assert_eq!(got.school(), tavernlab_core::cards::School::Nature);
}

#[test]
fn a_discover_only_offers_cards_the_engine_can_play() {
    // Offering an unimplemented card would put a dead draw in hand.
    use tavernlab_core::cards::is_implemented;
    let mut f = Fix::new();
    for _ in 0..8 {
        f.g.players[0].hand.clear();
        f.g.players[0].mana = 10; // each cast spends some
        f.play("Runed Orb", Some(Target::Hero(FOE)));
        if let Some(h) = f.g.players[0].hand.first() {
            assert!(is_implemented(h.card), "{} is not implemented", h.card.name());
        }
    }
}

#[test]
fn sawbones_spares_itself() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor", "Chillwind Yeti"])
        .deck(&["Wolfrider"]);
    f.play("Sawbones", None);
    let names: Vec<&str> = f.g.players[0].board.iter().map(|m| m.card.name()).collect();
    assert_eq!(names, vec!["Sawbones"], "everything else is destroyed");
    assert_eq!(f.g.players[0].hand.len(), 1, "and it drew");
}

#[test]
fn remnant_of_rage_cheapens_with_the_carnage() {
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor", "Bloodfen Raptor"]);
    let card = by_name("Remnant of Rage").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    assert_eq!(f.g.card_cost(ME, 0), card.def().cost, "nothing has died yet");

    f.play("Flamestrike", None);
    assert_eq!(f.g.deaths_this_turn, 2);
    assert_eq!(f.g.card_cost(ME, 0), card.def().cost - 2);
}

#[test]
fn the_death_count_resets_each_turn() {
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]);
    f.play("Flamestrike", None);
    assert_eq!(f.g.deaths_this_turn, 1);
    f.g.end_turn();
    f.g.current = FOE;
    f.g.begin_turn();
    assert_eq!(f.g.deaths_this_turn, 0);
}

// --------------------------------------------------------------- herald

#[test]
fn herald_summons_a_soldier_for_a_class_that_has_one() {
    let mut g = Game::new((Class::Warrior, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    assert_eq!(g.players[0].herald, 0);
    g.herald(ME);
    assert_eq!(g.players[0].herald, 1);
    assert_eq!(g.players[0].board.len(), 1);
    assert_eq!(g.players[0].board[0].card.name(), "Soldier of Ragnaros");
}

#[test]
fn a_class_with_no_soldier_still_advances_the_counter() {
    // Deathwing's cost reduction keys off the count, not the body.
    let mut g = Game::new((Class::Mage, &[]), (Class::Mage, &[]), 1).unwrap();
    g.herald(ME);
    assert_eq!(g.players[0].herald, 1);
    assert!(g.players[0].board.is_empty());
}

#[test]
fn the_soldier_scale_steps_at_two_and_four() {
    let mut g = Game::new((Class::Mage, &[]), (Class::Mage, &[]), 1).unwrap();
    assert_eq!(g.herald_scale(ME), 1);
    g.players[0].herald = 1;
    assert_eq!(g.herald_scale(ME), 1);
    g.players[0].herald = 2;
    assert_eq!(g.herald_scale(ME), 2);
    g.players[0].herald = 3;
    assert_eq!(g.herald_scale(ME), 2);
    g.players[0].herald = 4;
    assert_eq!(g.herald_scale(ME), 4);
}

#[test]
fn a_heralded_soldier_resolves_its_arrival_effect() {
    // Azshara gives the hero attack when it lands, scaled by the count.
    let mut g = Game::new((Class::DemonHunter, &[]), (Class::Mage, &[]), 1).unwrap();
    g.herald(ME);
    assert_eq!(g.players[0].hero_bonus_atk, 2, "scale 1 gives +2");
    g.players[0].hero_bonus_atk = 0;
    g.players[0].herald = 2;
    g.herald(ME);
    assert_eq!(g.players[0].hero_bonus_atk, 4, "scale 2 gives +4");
}

#[test]
fn preparation_discounts_only_the_next_spell() {
    let mut f = Fix::new().deck(&["Chillwind Yeti", "Chillwind Yeti"]);
    let intellect = by_name("Arcane Intellect").unwrap();
    f.play("Preparation", None);
    f.g.players[0].hand.push(HandCard::new(intellect));
    let i = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, i), intellect.def().cost - 2);

    let before = f.g.players[0].mana;
    f.g.apply(Action::Play { hand: i as u8, target: None, position: u8::MAX, choice: u8::MAX });
    assert_eq!(f.g.players[0].mana, before - (intellect.def().cost - 2));

    // Spent: a second spell pays full price.
    f.g.players[0].hand.push(HandCard::new(intellect));
    let j = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, j), intellect.def().cost);
}

#[test]
fn a_pending_discount_does_not_survive_the_turn() {
    let mut f = Fix::new();
    f.play("Preparation", None);
    assert_eq!(f.g.players[0].next_spell_discount, 2);
    f.g.end_turn();
    assert_eq!(f.g.players[0].next_spell_discount, 0);
}

#[test]
fn kaldorei_priestess_weakens_the_enemy_board_until_your_turn() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    f.play("Kaldorei Priestess", None);
    assert_eq!(f.theirs(0).atk, 2);
    // It wears off at the end of the enemy's turn — the turn before yours.
    f.g.current = FOE;
    f.g.end_turn();
    assert_eq!(f.theirs(0).atk, 4);
}

#[test]
fn eye_beam_is_cheaper_from_the_edge_of_the_hand() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    let beam = by_name("Eye Beam").unwrap();
    let filler = by_name("Chillwind Yeti").unwrap();
    f.g.players[0].hand.push(HandCard::new(beam));
    assert_eq!(f.g.card_cost(ME, 0), 1, "alone in hand it is both ends");

    f.g.players[0].hand.push(HandCard::new(filler));
    f.g.players[0].hand.push(HandCard::new(filler));
    assert_eq!(f.g.card_cost(ME, 0), 1, "leftmost still counts");

    f.g.players[0].hand.remove(0);
    f.g.players[0].hand.insert(1, HandCard::new(beam));
    assert_eq!(f.g.card_cost(ME, 1), beam.def().cost, "in the middle, full price");
}

#[test]
fn a_random_cost_summon_only_picks_that_cost() {
    let mut f = Fix::new();
    let made = f.g.summon_random_of_cost(ME, 3, 2);
    assert_eq!(made, 2);
    for m in f.g.players[0].board.iter() {
        assert_eq!(m.card.def().cost, 3);
        assert_eq!(m.card.def().kind(), tavernlab_core::cards::Kind::Minion);
    }
}

// ------------------------------------------------ tokens read from the card

#[test]
fn murloc_tidehunter_summons_the_token_the_card_names() {
    // The row does not name "Murloc Scout" anywhere: the card's own childIds
    // do. This is the test that the link survives the generator.
    let mut f = Fix::new();
    f.play("Murloc Tidehunter", None);
    assert_eq!(f.g.players[0].board.len(), 2, "the body and its Scout");
    assert_eq!(f.mine(1).card.name(), "Murloc Scout");
    assert_eq!((f.mine(1).atk, f.mine(1).health()), (1, 1));
}

#[test]
fn animal_companion_summons_one_of_the_three() {
    let mut f = Fix::new();
    f.play("Animal Companion", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    let name = f.mine(0).card.name();
    assert!(
        ["Huffer", "Leokk", "Misha"].contains(&name),
        "summoned {name}, which is not one of the Companions"
    );
}

#[test]
fn sanguine_infestation_draws_two_and_summons_two() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    let before = f.g.players[0].hand.len();
    f.play("Sanguine Infestation", None);
    assert_eq!(f.g.players[0].hand.len(), before + 2, "two cards drawn");
    assert_eq!(f.g.players[0].board.len(), 2, "two Leeches");
    for slot in 0..2 {
        assert_eq!(f.mine(slot).card.name(), "Bloated Leech");
        assert_eq!((f.mine(slot).atk, f.mine(slot).health()), (0, 2));
    }
}

// --------------------------------------------------------- the 2026 batch

#[test]
fn plated_beetle_gains_armor_when_it_dies() {
    let mut f = Fix::new().board(ME, &["Plated Beetle"]);
    f.g.players[0].armor = 0;
    f.g.deal_damage(Target::Minion(ME, 0), 9);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].armor, 3);
}

#[test]
fn bash_deals_three_and_gains_three_armor() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].armor = 0;
    f.play("Bash", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4);
    assert_eq!(f.g.players[0].armor, 3);
}

#[test]
fn lightning_bolt_overloads_its_caster() {
    // The row is only the damage; the Overload is in the table and the kernel
    // applies it. That is the half a text-parsed corpus used to get wrong.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Lightning Bolt", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4);
    assert_eq!(f.g.players[0].overload_next, 1, "Overload: (1)");
}

#[test]
fn flame_imp_burns_its_own_hero() {
    let mut f = Fix::new();
    f.play("Flame Imp", None);
    assert_eq!(f.g.players[0].hero_hp, 27);
    assert_eq!(f.g.players[1].hero_hp, 30, "the enemy is untouched");
}

#[test]
fn crimson_sigil_runner_draws_only_from_the_edge_of_the_hand() {
    // Played from a hand of one, it is both ends at once and Outcast applies.
    let mut f = Fix::new().deck(&["Wisp"]);
    let before = f.g.players[0].hand.len();
    f.play("Crimson Sigil Runner", None);
    assert_eq!(f.g.players[0].hand.len(), before + 1, "Outcast drew");
}

#[test]
fn hand_of_adal_buffs_and_draws() {
    let mut f = Fix::new().board(ME, &["Wisp"]).deck(&["Wisp"]);
    let before = f.g.players[0].hand.len();
    f.play("Hand of A'dal", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (3, 2), "a 1/1 plus 2/1");
    assert_eq!(f.g.players[0].hand.len(), before + 1);
}

#[test]
fn anti_magic_shell_buffs_and_protects_every_friendly_minion() {
    let mut f = Fix::new().board(ME, &["Wisp", "Wisp"]).board(FOE, &["Wisp"]);
    f.play("Anti-Magic Shell", None);
    for slot in 0..2 {
        assert_eq!((f.mine(slot).atk, f.mine(slot).health()), (2, 2));
        assert!(f.mine(slot).has(Keywords::ELUSIVE));
    }
    assert_eq!((f.theirs(0).atk, f.theirs(0).health()), (1, 1), "not the foe's");
}

#[test]
fn body_bagger_gains_a_corpse() {
    let mut f = Fix::new();
    f.g.players[0].corpses = 0;
    f.play("Body Bagger", None);
    assert_eq!(f.g.players[0].corpses, 1);
}

#[test]
fn frostbitten_imp_freezes_itself() {
    let mut f = Fix::new();
    f.play("Frostbitten Imp", None);
    assert!(f.mine(0).flags.has(Flags::FROZEN), "a 5/3 that cannot swing");
}

#[test]
fn a_dormant_body_needs_no_code_to_be_playable() {
    // Imprisoned Vilefiend is "Dormant for 2 turns. Rush" and nothing else.
    // The turn count is in the table and the kernel ticks it down, so the
    // card is understood without a behaviour row.
    let vilefiend = by_name("Imprisoned Vilefiend").expect("in corpus");
    assert!(behaviour_of(vilefiend).is_none(), "it needs no row");
    assert_eq!(vilefiend.def().dormant, 2);
    assert!(vilefiend.def().keywords.has(Keywords::RUSH));
}

#[test]
fn a_card_that_puts_others_to_sleep_is_not_dormant_itself() {
    // "make it go Dormant for 1 turn" is the same phrase, and reading it off
    // the text started Maiev's own body asleep.
    for name in ["Warden Maiev", "Maiev Shadowsong", "Demonic Confinement"] {
        let c = by_name(name).unwrap_or_else(|| panic!("no card {name}"));
        assert_eq!(c.def().dormant, 0, "{name} enters play awake");
    }
}

// ------------------------------------------ ported from the Python engine

#[test]
fn fire_fly_adds_its_elemental_to_hand() {
    let mut f = Fix::new();
    let before = f.g.players[0].hand.len();
    f.play("Fire Fly", None);
    assert_eq!(f.g.players[0].hand.len(), before + 1);
    let added = f.g.players[0].hand.last().unwrap().card;
    assert_eq!(added.name(), "Flame Elemental");
    assert_eq!((added.def().atk, added.def().hp), (1, 2));
}

#[test]
fn violet_spellwing_leaves_arcane_missiles_behind() {
    let mut f = Fix::new().board(ME, &["Violet Spellwing"]);
    let before = f.g.players[0].hand.len();
    f.g.deal_damage(Target::Minion(ME, 0), 9);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), before + 1);
    assert_eq!(
        f.g.players[0].hand.last().unwrap().card.name(),
        "Arcane Missiles"
    );
}

#[test]
fn contraband_wands_gives_three_copies() {
    let mut f = Fix::new();
    let before = f.g.players[0].hand.len();
    f.play("Contraband Wands", None);
    assert_eq!(f.g.players[0].hand.len(), before + 3);
    for hc in f.g.players[0].hand.iter().rev().take(3) {
        assert_eq!(hc.card.name(), "Arcane Missiles");
    }
}

#[test]
fn witchs_apprentice_adds_a_shaman_spell() {
    let mut f = Fix::new();
    let before = f.g.players[0].hand.len();
    f.play("Witch's Apprentice", None);
    assert_eq!(f.g.players[0].hand.len(), before + 1);
    let got = f.g.players[0].hand.last().unwrap().card;
    assert_eq!(got.def().kind(), tavernlab_core::cards::Kind::Spell);
    assert_eq!(got.def().class(), Class::Shaman, "{} is not a Shaman spell", got.name());
}

#[test]
fn carrier_whelp_adds_a_cheap_dragon() {
    let mut f = Fix::new();
    let before = f.g.players[0].hand.len();
    f.play("Carrier Whelp", None);
    assert_eq!(f.g.players[0].hand.len(), before + 1);
    let got = f.g.players[0].hand.last().unwrap().card;
    let d = got.def();
    assert_eq!(d.kind(), tavernlab_core::cards::Kind::Minion);
    assert!(d.races.any(tavernlab_core::cards::Races::DRAGON), "{}", got.name());
    assert!(d.cost <= 3, "{} costs {}", got.name(), d.cost);
}

#[test]
fn caged_cranium_grows_by_the_hand_it_left() {
    // Three other cards in hand, so the Cranium lands as a 2/(3+3).
    let mut f = Fix::new();
    for n in ["Wisp", "Wisp", "Wisp"] {
        let c = by_name(n).unwrap();
        f.g.players[0].hand.push(HandCard::new(c));
    }
    f.play("Caged Cranium", None);
    let printed = by_name("Caged Cranium").unwrap().def().hp;
    assert_eq!(f.mine(0).health(), printed + 3, "one Health per card left in hand");
}

#[test]
fn hijacked_securitybot_buffs_the_others_not_itself() {
    let mut f = Fix::new().board(ME, &["Wisp", "Wisp"]);
    f.play("Hijacked Securitybot", None);
    for slot in 0..2 {
        assert_eq!((f.mine(slot).atk, f.mine(slot).health()), (2, 2));
    }
    let bot = by_name("Hijacked Securitybot").unwrap().def();
    let me = f.mine(2);
    assert_eq!((me.atk, me.health()), (bot.atk, bot.hp), "the bot is unchanged");
}

#[test]
fn underbelly_network_summons_the_rat_the_card_prints() {
    // The Python original built this Rat by hand, deathrattle and all. Here it
    // comes from the card's own childIds, so it is the real printing.
    let mut f = Fix::new();
    f.play("Underbelly Network", None);
    let loc = f.g.players[0].board.len() - 1;
    let ok = f.g.apply(Action::UseLocation { slot: loc as u8, target: None });
    assert!(ok, "the Location refused to activate");
    let rat = f.g.players[0]
        .board
        .iter()
        .find(|m| m.card.name() == "Snoot Hoarder")
        .expect("no Rat on the board");
    assert_eq!((rat.atk, rat.max_hp), (2, 1));
    assert!(rat.has(Keywords::DEATHRATTLE) || behaviour_of(rat.card).is_some());
}

#[test]
fn gorishi_stinger_hits_and_leaves_a_grub() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Gorishi Stinger", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5);
    assert_eq!(f.mine(0).card.name(), "Silithid Grub");
    assert!(f.mine(0).has(Keywords::RUSH));
}

#[test]
fn infestation_hands_you_two_playable_stingers() {
    // The point of also implementing the Stinger: without it these would be
    // two cards that cost mana and do nothing.
    let mut f = Fix::new();
    let before = f.g.players[0].hand.len();
    f.play("Infestation", None);
    assert_eq!(f.g.players[0].hand.len(), before + 2);
    for hc in f.g.players[0].hand.iter().rev().take(2) {
        assert_eq!(hc.card.name(), "Gorishi Stinger");
        assert!(
            behaviour_of(hc.card).is_some(),
            "the Stinger it hands out is not implemented"
        );
    }
}

#[test]
fn secret_ingredient_first_mode_arms_the_hero() {
    let mut f = Fix::new();
    f.play_mode("Secret Ingredient", 0, None);
    assert_eq!(f.g.players[0].hero_attack(), 2);
}

#[test]
fn secret_ingredient_second_mode_gets_a_druid_card() {
    let mut f = Fix::new();
    let before = f.g.players[0].hand.len();
    f.play_mode("Secret Ingredient", 1, None);
    assert_eq!(f.g.players[0].hand.len(), before + 1);
    let got = f.g.players[0].hand.last().unwrap().card;
    assert_eq!(got.def().class(), Class::Druid, "{} is not a Druid card", got.name());
}

#[test]
fn morbid_swarm_first_mode_summons_two_ants() {
    let mut f = Fix::new();
    f.play_mode("Morbid Swarm", 0, None);
    assert_eq!(f.g.players[0].board.len(), 2);
    for slot in 0..2 {
        assert_eq!((f.mine(slot).atk, f.mine(slot).health()), (1, 1));
    }
}

#[test]
fn morbid_swarm_second_mode_spends_corpses_or_does_nothing() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].corpses = 2;
    f.play_mode("Morbid Swarm", 1, foe_minion(0));
    assert_eq!(f.theirs(0).health(), 3, "4 damage for 2 corpses");
    assert_eq!(f.g.players[0].corpses, 0);

    // With no corpses the mode must not go into debt.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].corpses = 1;
    f.play_mode("Morbid Swarm", 1, foe_minion(0));
    assert_eq!(f.theirs(0).health(), 7, "no corpses, no damage");
    assert_eq!(f.g.players[0].corpses, 1);
}

#[test]
fn felwood_treant_gives_a_temporary_crystal() {
    let mut f = Fix::new();
    f.g.players[0].mana = 3;
    f.play("Felwood Treant", None);
    // The Treant costs 2, so 3 - 2 + 1 = 2 left.
    assert_eq!(f.g.players[0].mana, 2);
}

#[test]
fn archmage_kalec_boosts_the_heros_spells() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Archmage Kalec", None);
    assert_eq!(f.g.players[0].spell_power(), 1);
    f.play("Frostbolt", foe_minion(0)); // 3 + 1
    assert_eq!(f.theirs(0).health(), 3);
}

#[test]
fn tricksy_improviser_needs_a_spell_cast_first() {
    // No spell this turn: the Battlecry does nothing.
    let mut f = Fix::new();
    f.play("Tricksy Improviser", None);
    assert_eq!(f.g.players[0].secrets.len(), 0);
}

#[test]
fn tricksy_improviser_arms_two_secrets_of_its_own_class() {
    let mut f = Fix::new();
    f.play("Arcane Intellect", None); // any spell will do
    f.play("Tricksy Improviser", None);
    assert_eq!(f.g.players[0].secrets.len(), 2);
    let mut names: Vec<&str> = f.g.players[0]
        .secrets
        .iter()
        .map(|c| {
            assert_eq!(c.def().class(), Class::Mage, "{} is not a Mage Secret", c.name());
            assert!(c.def().keywords.has(Keywords::SECRET));
            c.name()
        })
        .collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "the same Secret was armed twice");
}

#[test]
fn spells_cast_this_turn_resets_between_turns() {
    let mut f = Fix::new();
    f.play("Arcane Intellect", None);
    assert_eq!(f.g.players[0].spells_cast_turn, 1);
    assert_eq!(f.g.players[0].cards_played_turn, 1);
    f.play("Wisp", None);
    assert_eq!(f.g.players[0].spells_cast_turn, 1, "a minion is not a spell");
    assert_eq!(f.g.players[0].cards_played_turn, 2);
}

// -------------------------------------------- 2026 meta decks, phase 1

#[test]
fn staff_of_the_endbringer_destroys_all_minions_when_it_breaks() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor"])
        .board(FOE, &["Bloodfen Raptor"]);
    let staff = by_name("Staff of the Endbringer").unwrap();
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon {
        durability: 1,
        ..tavernlab_core::state::Weapon::equip(staff)
    });
    f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) });
    assert!(
        f.g.players[0].board.is_empty(),
        "the deathrattle clears the caster's own board too"
    );
    assert!(f.g.players[1].board.is_empty());
    assert!(f.g.players[0].weapon.is_none(), "the staff itself is gone");
}

#[test]
fn spiderling_gives_the_hero_plus_one_attack_on_its_controllers_turn_only() {
    let mut f = Fix::new()
        .board(ME, &["Spiderling"])
        .board(FOE, &["Spiderling"]);
    f.g.current = FOE;
    f.g.begin_turn();
    assert_eq!(f.g.players[0].hero_bonus_atk, 0, "not this player's turn");
    assert_eq!(f.g.players[1].hero_bonus_atk, 1, "but it is this one's");
}

#[test]
fn guard_dog_deathrattle_summons_a_one_cost_deathrattle_minion() {
    let mut f = Fix::new().board(ME, &["Guard Dog"]);
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(
        f.g.players[0].board.len(),
        1,
        "a random 1-Cost Deathrattle minion should replace it"
    );
    let summoned = f.g.players[0].board[0];
    assert_eq!(summoned.card.def().cost, 1);
    assert!(summoned.card.def().keywords.has(Keywords::DEATHRATTLE));
}

#[test]
fn earthen_roar_sets_health_to_one_with_no_second_target_by_default() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Chillwind Yeti"]); // 6/7, 4/5
    f.play("Earthen Roar", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 1);
    assert_eq!(f.theirs(1).health(), 5, "no Dragon in hand, no second target");
}

#[test]
fn earthen_roar_picks_the_highest_health_second_target_when_holding_a_dragon() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Chillwind Yeti"]); // 6/7, 4/5
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Winterspring Whelp").unwrap()));
    f.play("Earthen Roar", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 1);
    assert_eq!(f.theirs(1).health(), 1, "the other 3+ health enemy minion is also set to 1");
}

#[test]
fn cower_in_fear_damages_and_discounts_the_next_beast() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    f.play("Cower in Fear", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 2, "4/5 takes 3");
    assert_eq!(f.g.players[0].next_beast_discount, 2);

    let beast = by_name("Bloodfen Raptor").unwrap(); // a 2-cost Beast
    f.g.players[0].hand.push(HandCard::new(beast));
    let idx = f.g.players[0].hand.len() as u8 - 1;
    assert_eq!(f.g.card_cost(ME, idx as usize), 0, "2 cost minus a 2 discount");
    f.g.apply(Action::Play { hand: idx, target: None, position: u8::MAX, choice: u8::MAX });
    assert_eq!(f.g.players[0].next_beast_discount, 0, "spent on the Beast that used it");
}

#[test]
fn judgment_sets_every_minions_stats_to_the_chosen_ones() {
    let mut f = Fix::new()
        .board(ME, &["Wisp", "Chillwind Yeti"]) // 1/1, 4/5
        .board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Judgment", my_minion(1)); // the Chillwind Yeti, 4/5
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (4, 5));
    assert_eq!((f.mine(1).atk, f.mine(1).health()), (4, 5));
    assert_eq!((f.theirs(0).atk, f.theirs(0).health()), (4, 5));
}

#[test]
fn twilight_egg_hatches_a_one_one_whelp_if_it_dies_immediately() {
    let mut f = Fix::new().board(ME, &["Twilight Egg"]);
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (1, 1));
}

#[test]
fn twilight_egg_grows_a_turn_at_a_time_before_it_hatches() {
    let mut f = Fix::new().board(ME, &["Twilight Egg"]);
    f.g.begin_turn(); // the Egg's controller's own next turn: one growth tick
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(
        (f.mine(0).atk, f.mine(0).health()),
        (2, 2),
        "one turn of growth before it died"
    );
}

#[test]
fn soothsayer_deathrattle_heals_and_summons_a_six_cost_minion() {
    let mut f = Fix::new().board(ME, &["Soothsayer"]);
    f.g.players[0].hero_hp = 20;
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hero_hp, 26);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.def().cost, 6);
}

#[test]
fn hardlight_protector_heals_and_gives_its_hero_a_divine_shield() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play("Hardlight Protector", None);
    assert_eq!(f.g.players[0].hero_hp, 23);
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD), "the minion's own printed keyword");
    assert!(f.g.players[0].hero_divine_shield);

    assert!(!f.g.damage_hero(ME, 5), "the shield absorbs the hit");
    assert_eq!(f.g.players[0].hero_hp, 23, "no damage taken");
    assert!(!f.g.players[0].hero_divine_shield, "and pops");

    assert!(f.g.damage_hero(ME, 5), "no shield left the second time");
    assert_eq!(f.g.players[0].hero_hp, 18);
}

#[test]
fn intertwined_fate_discovers_a_copy_from_each_deck() {
    let mut f = Fix::new().deck(&["Bloodfen Raptor"]);
    f.g.players[1].deck.push(by_name("Chillwind Yeti").unwrap());
    f.play("Intertwined Fate", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "one pick from each deck");
    assert_eq!(f.g.players[1].deck.len(), 1, "the opponent keeps their copy");
    assert_eq!(f.g.players[0].deck.len(), 0, "the own-deck half relocates rather than copies");
}

#[test]
fn opu_the_unseen_battlecry_casts_fan_of_knives() {
    let mut f = Fix::new()
        .board(FOE, &["Wisp", "Wisp"]) // 1/1 each
        .deck(&["Bloodfen Raptor"]);
    f.play("Opu the Unseen", None);
    assert_eq!(f.their_board(), 0, "Fan of Knives clears 1-health minions on battlecry");
    assert_eq!(f.g.players[0].hand.len(), 1, "and draws");
}

#[test]
fn opu_the_unseen_deathrattle_also_casts_fan_of_knives() {
    let mut f = Fix::new().board(ME, &["Opu the Unseen"]).board(FOE, &["Wisp"]);
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0, "the deathrattle's Fan of Knives kills the 1-health Wisp");
}

#[test]
fn agent_of_the_old_ones_transforms_its_priciest_hand_card_into_a_coin() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Boulderfist Ogre").unwrap()));
    f.play("Agent of the Old Ones", None);
    let names: Vec<&str> = f.g.players[0].hand.iter().map(|hc| hc.card.name()).collect();
    assert!(names.contains(&"Wisp"), "the cheap card stays");
    assert!(names.contains(&"The Coin"), "the priciest card became a Coin");
    assert!(!names.contains(&"Boulderfist Ogre"));
}

#[test]
fn deja_vu_discovers_a_copy_from_the_opponents_hand() {
    let mut f = Fix::new();
    f.g.players[1]
        .hand
        .push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.play("Deja Vu", None);
    assert_eq!(
        f.g.players[0]
            .hand
            .iter()
            .filter(|hc| hc.card.name() == "Chillwind Yeti")
            .count(),
        1
    );
    assert_eq!(f.g.players[1].hand.len(), 1, "the opponent keeps their card");
}

#[test]
fn cultist_map_discovers_from_the_own_deck() {
    let mut f = Fix::new().deck(&["Bloodfen Raptor"]);
    f.play("Cultist Map", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].deck.len(), 0);
}

#[test]
fn getaway_hogdriver_gains_charge_when_both_draws_are_minions() {
    let mut f = Fix::new().deck(&["Bloodfen Raptor", "Chillwind Yeti"]);
    f.play("Getaway Hogdriver", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Getaway Hogdriver")
        .unwrap();
    assert!(f.g.players[0].board[slot].has(Keywords::CHARGE));
    assert_eq!(f.g.players[0].hand.len(), 2, "both draws stayed in hand");
}

#[test]
fn getaway_hogdriver_stays_summoning_sick_if_a_spell_was_drawn() {
    let mut f = Fix::new().deck(&["Bloodfen Raptor", "Fireball"]);
    f.play("Getaway Hogdriver", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Getaway Hogdriver")
        .unwrap();
    assert!(!f.g.players[0].board[slot].has(Keywords::CHARGE));
}

#[test]
fn cursed_catacombs_discovers_from_the_deck_for_free() {
    let mut f = Fix::new().deck(&["Bloodfen Raptor"]);
    f.play("Cursed Catacombs", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn eredar_deceptor_summons_a_demon_each_time_its_controller_draws() {
    let mut f = Fix::new()
        .board(ME, &["Eredar Deceptor"])
        .deck(&["Bloodfen Raptor"]);
    f.g.draw_cards(ME, 1);
    assert_eq!(
        f.g.players[0].board.len(),
        2,
        "the Eredar Deceptor plus a summoned Demon"
    );
    let summoned = f.g.players[0].board[1];
    assert!(summoned.races().any(tavernlab_core::cards::Races::DEMON));
    assert!(summoned.has(Keywords::RUSH));
}

#[test]
fn eredar_deceptor_does_not_react_to_the_opponent_drawing() {
    let mut f = Fix::new().board(ME, &["Eredar Deceptor"]);
    f.g.players[1].deck.push(by_name("Bloodfen Raptor").unwrap());
    f.g.draw_cards(FOE, 1);
    assert_eq!(f.g.players[0].board.len(), 1, "no reaction to the opponent's draw");
}

#[test]
fn brood_keeper_equips_a_sword_only_when_holding_a_dragon() {
    let mut f = Fix::new();
    f.play("Brood Keeper", None);
    assert!(f.g.players[0].weapon.is_none(), "no Dragon in hand");
}

#[test]
fn brood_keeper_equips_when_holding_a_dragon() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Winterspring Whelp").unwrap()));
    f.play("Brood Keeper", None);
    let w = f.g.players[0].weapon.expect("should have equipped a sword");
    assert_eq!((w.atk, w.durability), (2, 2));
}

#[test]
fn stadium_announcer_equips_both_players_and_buffs_its_own() {
    let mut f = Fix::new();
    f.play("Stadium Announcer", None);
    let mine = f.g.players[0].weapon.expect("should have equipped a weapon");
    assert!(f.g.players[1].weapon.is_some(), "the opponent should have equipped too");
    assert_eq!(mine.atk, mine.card.def().atk + 1, "the caster's own weapon gets +1/+1");
    assert_eq!(mine.durability, mine.card.def().dur + 1);
}

#[test]
fn erupting_volcano_deals_three_with_no_fire_spell_this_turn() {
    let mut f = Fix::new(); // no enemy minions: the whole split lands on the hero
    f.play("Erupting Volcano", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Erupting Volcano")
        .unwrap() as u8;
    f.g.apply(Action::UseLocation { slot, target: None });
    assert_eq!(f.g.players[1].hero_hp, 27, "3 damage split with nowhere else to go");
}

#[test]
fn erupting_volcano_deals_six_after_a_fire_spell_this_turn() {
    let mut f = Fix::new();
    f.play("Fireball", Some(Target::Hero(FOE))); // a Fire spell, 30 -> 24
    f.play("Erupting Volcano", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Erupting Volcano")
        .unwrap() as u8;
    f.g.apply(Action::UseLocation { slot, target: None });
    assert_eq!(f.g.players[1].hero_hp, 18, "6 damage after the +3 Fire bonus, 24 -> 18");
}

#[test]
fn torch_deals_eight_to_a_damaged_minion_ignoring_spell_power() {
    let mut f = Fix::new()
        .board(ME, &["Kobold Geomancer"]) // Spell Damage +1
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[1].board[0].max_hp = 20; // tanky enough to inspect afterward
    f.g.players[1].board[0].damage = 1; // damaged, so it is a legal target
    f.play("Torch", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 9, "1 existing + a flat 8, unaffected by Spell Power");
}

#[test]
fn darkrider_discovers_a_dragon_only_when_holding_one() {
    let mut f = Fix::new();
    f.play("Darkrider", None);
    assert_eq!(f.g.players[0].hand.len(), 0, "no Dragon in hand, no Discover");
}

#[test]
fn darkrider_discovers_when_holding_a_dragon() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Winterspring Whelp").unwrap()));
    f.play("Darkrider", None);
    let discovered = f.g.players[0].hand.last().unwrap().card;
    assert!(discovered.def().races.any(tavernlab_core::cards::Races::DRAGON));
}

#[test]
fn shadowflame_suffusion_deals_two_and_discovers_a_warrior_minion() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    f.play("Shadowflame Suffusion", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 3);
    let discovered = f.g.players[0].hand.last().unwrap().card;
    assert_eq!(discovered.def().class(), tavernlab_core::cards::Class::Warrior);
}

#[test]
fn dark_bribe_draws_three_and_gives_the_cheapest_to_the_opponent() {
    let mut f = Fix::new().deck(&["Wisp", "Chillwind Yeti", "Boulderfist Ogre"]); // 0, 4, 6 cost
    f.play("Dark Bribe", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "drew 3, gave the cheapest away");
    assert!(
        !f.g.players[0].hand.iter().any(|hc| hc.card.name() == "Wisp"),
        "the 0-cost Wisp was the cheapest and went to the opponent"
    );
    assert_eq!(f.g.players[1].hand.len(), 1);
    assert_eq!(f.g.players[1].hand[0].card.name(), "Wisp");
}

// -------------------------------------------------------------------- G12

#[test]
fn imp_gang_stooge_deathrattle_puts_two_demons_on_the_bottom_of_the_deck() {
    let mut f = Fix::new().board(ME, &["Imp Gang Stooge"]);
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].deck.len(), 2);
    for card in f.g.players[0].deck.iter() {
        assert_eq!(card.name(), "Grandmother Imp");
        assert_eq!((card.def().atk, card.def().hp), (8, 8));
        assert!(card.def().keywords.has(Keywords::TAUNT));
        assert!(card.def().keywords.has(Keywords::LIFESTEAL));
    }
}

#[test]
fn annihilation_destroys_all_minions_and_summons_demons_from_the_bottom_three() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor"])
        .board(FOE, &["Chillwind Yeti"])
        // Bottom three (index 0 first): only Voidwalker is a Demon.
        .deck(&["Voidwalker", "Wisp", "Chillwind Yeti"]);
    f.play("Annihilation", None);
    assert_eq!(f.their_board(), 0, "the enemy Yeti died too");
    assert_eq!(f.g.players[0].board.len(), 1, "the Raptor died, Voidwalker was summoned");
    assert_eq!(f.mine(0).card.name(), "Voidwalker");
    assert_eq!(f.g.players[0].deck.len(), 2, "Voidwalker left the deck, the other two stayed");
}

#[test]
fn cosmic_manifestations_deals_damage_once_when_not_outcast() {
    let mut f = Fix::new();
    let filler = by_name("Wisp").unwrap();
    let cosmic = by_name("Cosmic Manifestations").unwrap();
    f.g.players[0].hand.push(HandCard::new(filler));
    f.g.players[0].hand.push(HandCard::new(cosmic)); // the middle of three: not Outcast
    f.g.players[0].hand.push(HandCard::new(filler));
    let ok = f.g.apply(Action::Play {
        hand: 1,
        target: Some(Target::Hero(FOE)),
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert!(ok);
    assert_eq!(f.g.players[1].hero_hp, 28, "2 damage, once");
    assert_eq!(f.g.players[0].deck.len(), 1, "one Demon Hunter spell shuffled in");
    assert_eq!(
        f.g.players[0].deck[0].def().class(),
        tavernlab_core::cards::Class::DemonHunter
    );
}

#[test]
fn cosmic_manifestations_outcast_does_it_twice() {
    // A one-card hand is Outcast on both ends at once.
    let mut f = Fix::new();
    f.play("Cosmic Manifestations", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 26, "2 damage, twice");
    assert_eq!(f.g.players[0].deck.len(), 2, "a Demon Hunter spell shuffled in, twice");
    for card in f.g.players[0].deck.iter() {
        assert_eq!(card.def().class(), tavernlab_core::cards::Class::DemonHunter);
    }
}

// --------------------------------------------------------------- G1 / G2

#[test]
fn acceleration_aura_grants_a_temp_crystal_for_the_next_three_turns_only() {
    let mut f = Fix::new();
    f.play("Acceleration Aura", None);
    assert_eq!(f.g.players[0].mana, 8, "spent its own cost of 2, no bonus crystal yet");
    assert_eq!(f.g.players[0].pending.len(), 1);
    for _ in 0..3 {
        f.g.begin_turn();
        assert_eq!(f.g.players[0].mana, 11, "a temp crystal on top of the base 10");
    }
    assert_eq!(f.g.players[0].pending.len(), 0, "spent after three turns");
    f.g.begin_turn();
    assert_eq!(f.g.players[0].mana, 10, "no more bonus on the fourth turn");
}

#[test]
fn sigil_of_the_seas_summons_a_naga_next_turn_only() {
    let mut f = Fix::new();
    f.play("Sigil of the Seas", None);
    assert_eq!(f.g.players[0].board.len(), 0, "no immediate summon");
    f.g.begin_turn();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Naga Monstrosity");
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (3, 3));
    assert!(f.mine(0).has(Keywords::TAUNT));
    f.g.begin_turn();
    assert_eq!(f.g.players[0].board.len(), 1, "one-shot: no second Naga");
}

#[test]
fn rotten_apple_heals_now_and_hurts_for_the_next_two_turns() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    f.g.players[0].hero_hp = 10;
    f.play("Rotten Apple", None);
    assert_eq!(f.g.players[0].hero_hp, 22, "healed 12 immediately, no damage yet");
    f.g.begin_turn();
    assert_eq!(f.g.players[0].hero_hp, 19, "3 damage on the first of the next two turns");
    f.g.begin_turn();
    assert_eq!(f.g.players[0].hero_hp, 16, "and the second");
    f.g.begin_turn();
    assert_eq!(f.g.players[0].hero_hp, 16, "spent after two turns");
}

#[test]
fn cult_neophyte_taxes_the_opponents_spells_on_their_own_next_turn_only() {
    let mut f = Fix::new();
    f.play("Cult Neophyte", None);
    assert_eq!(f.g.players[1].spell_tax_pending, 1);
    assert_eq!(f.g.players[1].spell_tax_active, 0, "not active yet");

    f.g.current = FOE;
    f.g.begin_turn();
    assert_eq!(f.g.players[1].spell_tax_active, 1, "active on the opponent's own next turn");
    let fireball = by_name("Fireball").unwrap();
    f.g.players[1].hand.push(HandCard::new(fireball));
    let idx = f.g.players[1].hand.len() - 1;
    assert_eq!(f.g.card_cost(FOE, idx), fireball.def().cost + 1);

    f.g.current = ME;
    f.g.begin_turn();
    f.g.current = FOE;
    f.g.begin_turn();
    assert_eq!(f.g.players[1].spell_tax_active, 0, "expired the turn after");
}

#[test]
fn ursol_casts_its_priciest_untargeted_hand_spell_once() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap())); // filler, not a spell
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Arcane Intellect").unwrap())); // draw 2
    f.play("Ursol", None);
    assert_eq!(
        f.g.players[0].hand.len(),
        3,
        "the Wisp filler plus the two cards Arcane Intellect drew"
    );
    assert!(
        !f.g.players[0].hand.iter().any(|hc| hc.card.name() == "Arcane Intellect"),
        "the chosen spell left the hand"
    );
}

#[test]
fn soulrest_ceremony_buffs_and_rushes_then_kills_them_at_end_of_turn() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]); // 4/5
    f.play("Soulrest Ceremony", None);
    assert_eq!(f.mine(0).atk, 5, "+1 attack");
    assert!(f.mine(0).has(Keywords::RUSH));
    assert_eq!(f.g.players[0].board.len(), 1, "still alive mid-turn");
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 0, "dies at the end of the turn");
}

#[test]
fn platysaur_battlecry_draws_and_marks_the_drawn_card() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("Platysaur", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "the drawn Wisp");
    assert!(
        f.g.players[0].hand[0]
            .marks
            .has(tavernlab_core::state::Marks::DRAWN_BY_PLATYSAUR)
    );
}

#[test]
fn platysaur_deathrattle_discards_only_the_card_it_drew() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("Platysaur", None); // battlecry draws and marks the Wisp
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Chillwind Yeti").unwrap())); // unmarked
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Platysaur")
        .unwrap();
    f.g.destroy(Target::Minion(ME, slot as u8));
    f.g.sweep_deaths();
    let names: Vec<&str> = f.g.players[0].hand.iter().map(|hc| hc.card.name()).collect();
    assert_eq!(names, vec!["Chillwind Yeti"], "only the marked Wisp was discarded");
}

#[test]
fn ebb_and_flow_needs_a_minion_played_first_for_the_armor() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Ebb and Flow", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "3 damage");
    assert_eq!(f.g.players[0].armor, 0, "no minion played yet");
}

#[test]
fn ebb_and_flow_gains_armor_after_a_minion_was_played_while_held() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Ebb and Flow").unwrap()));
    f.play("Wisp", None); // a minion, played while Ebb and Flow sits in hand
    let idx = f.g.players[0]
        .hand
        .iter()
        .position(|hc| hc.card.name() == "Ebb and Flow")
        .unwrap();
    let ok = f.g.apply(Action::Play {
        hand: idx as u8,
        target: foe_minion(0),
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert!(ok);
    assert_eq!(f.g.players[0].armor, 5);
}

#[test]
fn mind_sweeper_needs_an_opponents_card_played_while_held() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp"]);
    f.play("Mind Sweeper", None);
    assert_eq!(f.their_board(), 2, "no opponent's card played yet, no damage");
}

#[test]
fn mind_sweeper_deals_damage_after_playing_an_opponents_card() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Mind Sweeper").unwrap()));
    f.play("Voidwalker", None); // a Warlock card in a Mage's hand: picked up somehow
    let idx = f.g.players[0]
        .hand
        .iter()
        .position(|hc| hc.card.name() == "Mind Sweeper")
        .unwrap();
    f.g.apply(Action::Play {
        hand: idx as u8,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert_eq!(f.their_board(), 0, "both 1-health Wisps die to the 2 damage");
}

#[test]
fn chainbreaker_hogger_duplicates_other_legendaries_at_start_of_game() {
    use tavernlab_core::agent::{Scripted, Style};
    use tavernlab_core::cards::Rarity;

    let hogger = by_name("Chainbreaker Hogger").unwrap();
    let leg = tavernlab_core::cards::all()
        .find(|c| c.def().rarity() == Rarity::Legendary && c.def().collectible && *c != hogger)
        .expect("the corpus has another Legendary");
    let filler = by_name("Wisp").unwrap();
    let mut deck = vec![hogger, leg];
    deck.resize(30, filler);

    let mut g = Game::new((Class::Warrior, &deck), (Class::Warrior, &deck), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    let count = g.players[0].deck.iter().filter(|&&c| c == leg).count()
        + g.players[0].hand.iter().filter(|hc| hc.card == leg).count();
    assert_eq!(count, 2, "the other Legendary should now appear twice total");
}

#[test]
fn king_llane_hides_in_the_opponents_deck_at_start_of_game() {
    use tavernlab_core::agent::{Scripted, Style};

    let llane = by_name("King Llane").unwrap();
    let filler = by_name("Wisp").unwrap();
    let mut deck = vec![llane];
    deck.resize(30, filler);
    let empty_deck = vec![filler; 30];

    let mut g = Game::new((Class::Rogue, &deck), (Class::Mage, &empty_deck), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    let mine = g.players[0].deck.iter().filter(|&&c| c == llane).count()
        + g.players[0].hand.iter().filter(|hc| hc.card == llane).count();
    let theirs = g.players[1].deck.iter().filter(|&&c| c == llane).count()
        + g.players[1].hand.iter().filter(|hc| hc.card == llane).count();
    assert_eq!(mine, 0, "no longer anywhere in its owner's zones");
    assert_eq!(theirs, 1, "planted in the opponent's deck instead");
}

#[test]
fn king_llane_battlecry_draws_and_shuffles_itself_back() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("King Llane", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "drew the Wisp");
    assert!(
        f.g.players[0].deck.iter().any(|&c| c.name() == "King Llane"),
        "shuffled itself back into the deck it was played from"
    );
}

#[test]
fn garona_halforcen_destroys_king_llane_and_halves_health_if_opponent_holds_it() {
    let mut f = Fix::new();
    f.g.players[1]
        .hand
        .push(HandCard::new(by_name("King Llane").unwrap()));
    f.g.players[1].hero_hp = 21;
    f.play("Garona Halforcen", None);
    assert!(!f.g.players[1].hand.iter().any(|hc| hc.card.name() == "King Llane"));
    assert_eq!(f.g.players[1].hero_hp, 10, "halved, rounding down");
}

#[test]
fn garona_halforcen_does_nothing_without_king_llane() {
    let mut f = Fix::new();
    f.g.players[1].hero_hp = 21;
    f.play("Garona Halforcen", None);
    assert_eq!(f.g.players[1].hero_hp, 21);
}

#[test]
fn emergency_surgery_summons_four_lifesteal_undead_that_attack_the_target() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[1].board[0].atk = 0; // isolate: no counter-damage back
    f.g.players[1].board[0].max_hp = 20;
    f.g.players[0].hero_hp = 10;
    f.play("Emergency Surgery", foe_minion(0));
    assert_eq!(f.g.players[0].board.len(), 4, "four Necronurses");
    assert!(f.g.players[0].board.iter().all(|m| m.card.name() == "Necronurse"));
    assert_eq!(f.theirs(0).damage, 12, "4 x 3 damage");
    assert_eq!(f.g.players[0].hero_hp, 22, "4 x 3 Lifesteal");
}

#[test]
fn spire_of_solitude_summons_a_demon_sized_by_hand_and_attacks_a_random_enemy() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[1].board[0].atk = 0; // isolate: no counter-damage back
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Spire of Solitude", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Spire of Solitude")
        .unwrap() as u8;
    f.g.apply(Action::UseLocation { slot, target: None });
    let demon = f.g.players[0]
        .board
        .iter()
        .find(|m| m.card.name() == "Shivarra Infiltrator")
        .expect("the Demon should have been summoned");
    assert_eq!((demon.atk, demon.health()), (2, 2), "sized to the 2-card hand");
    assert_eq!(f.theirs(0).damage, 2, "attacked the only enemy minion");
}

#[test]
fn the_food_chain_completes_after_all_four_attack_thresholds() {
    let mut f = Fix::new();
    f.play("The Food Chain", None);
    assert!(f.g.players[0].quest.is_some());

    for atk in [1, 3, 5, 7] {
        let beast = tavernlab_core::cards::all()
            .find(|c| {
                c.def().kind() == tavernlab_core::cards::Kind::Minion
                    && c.def().races.any(tavernlab_core::cards::Races::BEAST)
                    && c.def().atk == atk
                    && c.def().collectible
            })
            .unwrap_or_else(|| panic!("need a {atk}-Attack Beast in the corpus"));
        f.g.players[0].mana = 10;
        f.play(beast.name(), None);
    }
    assert!(f.g.players[0].quest.is_none(), "completed and cleared");
    assert!(
        f.g.players[0]
            .hand
            .iter()
            .any(|hc| hc.card.name() == "Shokk, Jungle Tyrant"),
        "reward in hand"
    );
}

#[test]
fn unleash_the_colossus_completes_after_twelve_hits_of_exactly_two() {
    let mut f = Fix::new();
    f.play("Unleash the Colossus", None);
    assert!(f.g.players[0].quest.is_some());
    for _ in 0..12 {
        f.g.deal_damage(Target::Hero(FOE), 2);
    }
    assert!(f.g.players[0].quest.is_none(), "completed and cleared");
    assert!(
        f.g.players[0]
            .hand
            .iter()
            .any(|hc| hc.card.name() == "Gorishi Colossus"),
        "reward in hand"
    );
}

#[test]
fn unleash_the_colossus_ignores_damage_that_is_not_exactly_two() {
    let mut f = Fix::new();
    f.play("Unleash the Colossus", None);
    for _ in 0..12 {
        f.g.deal_damage(Target::Hero(FOE), 3);
    }
    assert_eq!(f.g.players[0].quest.unwrap().1, 0, "wrong amount never counts");
}

#[test]
fn storm_the_gates_completes_after_three_beasts_or_undead() {
    let mut f = Fix::new();
    f.play("Storm the Gates", None);
    assert!(f.g.players[0].sidequest.is_some());
    for _ in 0..3 {
        f.g.players[0].mana = 10;
        f.play("Bloodfen Raptor", None); // a Beast
    }
    assert!(f.g.players[0].sidequest.is_none(), "completed and cleared");
    assert!(
        f.g.players[0].hand.iter().any(|hc| hc.card.name() == "Zombeast"),
        "reward in hand"
    );
}

#[test]
fn the_egg_of_khelos_hatches_through_all_five_stages() {
    let mut f = Fix::new().board(ME, &["The Egg of Khelos"]);
    for _ in 0..5 {
        let slot = f.g.players[0]
            .board
            .iter()
            .position(|m| m.card.name() == "The Egg of Khelos")
            .expect("an Egg should still be on board");
        f.g.destroy(Target::Minion(ME, slot as u8));
        f.g.sweep_deaths();
    }
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Khelos");
    assert_eq!((f.mine(0).atk, f.mine(0).health()), (20, 20));
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn reanimated_pterrordax_costs_corpses_not_mana() {
    let mut f = Fix::new();
    f.g.players[0].mana = 0;
    f.g.players[0].corpses = 5;
    f.play("Reanimated Pterrordax", None);
    assert_eq!(f.g.players[0].corpses, 0, "spent 5 corpses");
    assert_eq!(f.g.players[0].mana, 0, "no mana spent");
    assert_eq!(f.g.players[0].board.len(), 1);
}

#[test]
fn reanimated_pterrordax_is_unplayable_without_enough_corpses() {
    let mut f = Fix::new();
    f.g.players[0].corpses = 4; // one short of the 5 it needs
    let card = by_name("Reanimated Pterrordax").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let idx = f.g.players[0].hand.len() as u8 - 1;
    let ok = f.g.apply(Action::Play {
        hand: idx,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert!(!ok, "not enough corpses");
    assert_eq!(f.g.players[0].corpses, 4, "nothing spent on the failed attempt");
}

#[test]
fn blood_doctor_thalena_grants_an_independent_second_hero_power() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Blood Doctor Thal'ena", None);
    assert_eq!(
        f.g.players[0].second_hero_power,
        Some(by_name("Vampyr's Kiss").unwrap())
    );

    // The class hero power (Fireblast, Mage) still works independently.
    assert!(f.g.apply(Action::HeroPower {
        target: Some(Target::Hero(FOE)),
        second: false,
    }));
    assert_eq!(f.g.players[1].hero_hp, 29);

    f.g.players[0].corpses = 3;
    assert!(f.g.apply(Action::HeroPower {
        target: my_minion(0),
        second: true,
    }));
    assert_eq!(f.g.players[0].corpses, 0, "paid in Corpses, not Mana");
    assert_eq!(f.mine(0).atk, 4, "+3 Attack from Vampyr's Kiss");
}

#[test]
fn shaladrassil_gives_the_five_plain_dream_cards_by_default() {
    let mut f = Fix::new();
    f.play("Shaladrassil", None);
    let names: Vec<&str> = f.g.players[0].hand.iter().map(|hc| hc.card.name()).collect();
    for n in ["Nightmare", "Dream", "Laughing Sister", "Ysera Awakens", "Emerald Drake"] {
        assert!(names.contains(&n), "missing {n}");
    }
}

#[test]
fn shaladrassil_corrupts_them_after_a_higher_cost_card_was_played_while_held() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Shaladrassil").unwrap()));
    let pricier = tavernlab_core::cards::all()
        .find(|c| {
            c.def().cost > 8
                && c.def().collectible
                && c.def().kind() == tavernlab_core::cards::Kind::Minion
        })
        .expect("need a Minion costing more than Shaladrassil's own 8");
    f.g.players[0].mana = 10;
    f.play(pricier.name(), None);

    let idx = f.g.players[0]
        .hand
        .iter()
        .position(|hc| hc.card.name() == "Shaladrassil")
        .unwrap();
    f.g.players[0].mana = 10;
    let ok = f.g.apply(Action::Play {
        hand: idx as u8,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert!(ok);
    let names: Vec<&str> = f.g.players[0].hand.iter().map(|hc| hc.card.name()).collect();
    for n in [
        "Corrupted Nightmare",
        "Corrupted Dream",
        "Corrupted Laughing Sister",
        "Corrupted Awakening",
        "Corrupted Drake",
    ] {
        assert!(names.contains(&n), "missing {n}");
    }
}

#[test]
fn warptooth_summons_itself_from_hand_after_four_friendly_damage_instances() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]); // 4/5
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Warptooth").unwrap()));
    for _ in 0..4 {
        f.g.deal_damage(Target::Minion(ME, 0), 1);
    }
    assert!(
        !f.g.players[0].hand.iter().any(|hc| hc.card.name() == "Warptooth"),
        "left the hand"
    );
    assert!(
        f.g.players[0].board.iter().any(|m| m.card.name() == "Warptooth"),
        "summoned onto the board"
    );
}

#[test]
fn warptooth_summons_itself_from_the_deck_too() {
    let mut f = Fix::new();
    f.g.players[0].deck.push(by_name("Warptooth").unwrap());
    for _ in 0..4 {
        f.g.deal_damage(Target::Hero(ME), 1);
    }
    assert!(
        !f.g.players[0].deck.iter().any(|&c| c.name() == "Warptooth"),
        "left the deck"
    );
    assert!(f.g.players[0].board.iter().any(|m| m.card.name() == "Warptooth"));
}

#[test]
fn warptooth_does_not_count_damage_to_the_opponent() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Warptooth").unwrap()));
    for _ in 0..4 {
        f.g.deal_damage(Target::Hero(FOE), 3);
    }
    assert_eq!(f.g.players[0].friendly_damaged_turn, 0);
    assert!(f.g.players[0].hand.iter().any(|hc| hc.card.name() == "Warptooth"));
}

#[test]
fn unshackle_soul_costs_one_after_playing_an_opponents_card() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    let unshackle = by_name("Unshackle Soul").unwrap();
    f.g.players[0].hand.push(HandCard::new(unshackle));
    let before_idx = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, before_idx), unshackle.def().cost, "full price so far");

    f.play("Voidwalker", None); // a Warlock card in a Mage's hand: picked up somehow
    let idx = f.g.players[0]
        .hand
        .iter()
        .position(|hc| hc.card.name() == "Unshackle Soul")
        .unwrap();
    assert_eq!(f.g.card_cost(ME, idx), 1);
    let ok = f.g.apply(Action::Play {
        hand: idx as u8,
        target: foe_minion(0),
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert!(ok);
    assert_eq!(f.their_board(), 0);
}

#[test]
fn elise_the_navigator_plays_as_a_vanilla_body() {
    let mut f = Fix::new();
    f.play("Elise the Navigator", None);
    assert_eq!(f.mine(0).atk, 3);
    assert_eq!(f.mine(0).max_hp, 5);
}

#[test]
fn bashana_runetotem_gets_three_bare_treants() {
    let mut f = Fix::new();
    f.play("Bashana Runetotem", None);
    assert_eq!(f.g.players[0].board.len(), 4, "Bashana plus three Treants");
    for slot in 1..4 {
        assert_eq!(f.mine(slot).card.name(), "Treant");
        assert_eq!(f.mine(slot).atk, 2);
        assert_eq!(f.mine(slot).max_hp, 2);
    }
}

#[test]
fn toreth_the_unbreaking_plays_with_its_printed_keywords() {
    let mut f = Fix::new();
    f.play("Toreth the Unbreaking", None);
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn tiny_pal_deals_one_to_all_enemies_after_the_hero_attacks() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Tiny Pal").unwrap(),
    ));
    f.g.apply(Action::HeroAttack {
        target: Target::Hero(FOE),
    });
    assert_eq!(
        f.theirs(0).max_hp - f.theirs(0).damage,
        4,
        "the Yeti took 1 from the fixed ammo"
    );
    assert_eq!(
        f.g.players[1].hero_hp,
        30 - 2 - 1,
        "the hero took 2 from the swing and 1 from the ammo"
    );
}

#[test]
fn nightmare_fuel_discovers_a_minion_copy_from_the_opponents_deck() {
    let mut f = Fix::new();
    f.g.players[1].deck.push(by_name("Chillwind Yeti").unwrap());
    f.play("Nightmare Fuel", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Chillwind Yeti");
    assert_eq!(
        f.g.players[1].deck.len(),
        1,
        "the opponent keeps their copy"
    );
}

#[test]
fn dreambound_raptor_gives_a_fixed_bonus_but_not_to_itself() {
    let mut f = Fix::new().board(ME, &["Dreambound Raptor"]);
    f.play("Chillwind Yeti", None);
    assert_eq!(f.mine(1).atk, 5, "4/5 Yeti plus the fixed +1/+1");
    assert_eq!(f.mine(1).max_hp, 6);
    assert_eq!(f.mine(0).atk, 2, "the Raptor itself is unaffected");
    assert_eq!(f.mine(0).max_hp, 1);
}

#[test]
fn the_fins_beyond_time_swaps_to_the_starting_hand_and_back() {
    let wisp = by_name("Wisp").unwrap();
    let raptor = by_name("Bloodfen Raptor").unwrap();
    let mut f = Fix::new();
    f.g.players[0].starting_hand.push(wisp);
    f.g.players[0].starting_hand.push(wisp);
    f.g.players[0].hand.push(HandCard::new(raptor));

    f.play("The Fins Beyond Time", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "now the two Wisps");
    assert!(f.g.players[0].hand.iter().all(|hc| hc.card == wisp));
    let stash = f.g.players[0]
        .swapped_hand
        .expect("the real hand was stashed");
    assert_eq!(
        stash.len(),
        1,
        "just the Raptor -- Fins had already left hand"
    );
    assert_eq!(stash[0].card, raptor);

    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1, "swapped back");
    assert_eq!(f.g.players[0].hand[0].card, raptor);
    assert!(
        f.g.players[0].swapped_hand.is_none(),
        "cleared once restored"
    );
}

#[test]
fn mugzee_grants_zees_might_with_no_spells_in_deck() {
    use tavernlab_core::agent::{Scripted, Style};

    let mugzee = by_name("Mug'Zee").unwrap();
    let wisp = by_name("Wisp").unwrap();
    let mut deck = vec![mugzee];
    deck.resize(30, wisp);

    let mut g = Game::new((Class::Shaman, &deck), (Class::Shaman, &deck), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    assert_eq!(g.players[0].hero_power.name(), "Zee's Might");
}

#[test]
fn mugzee_grants_mugs_magic_with_no_other_minions_in_deck() {
    use tavernlab_core::agent::{Scripted, Style};

    let mugzee = by_name("Mug'Zee").unwrap();
    let bolt = by_name("Lightning Bolt").unwrap();
    let mut deck = vec![mugzee];
    deck.resize(30, bolt);

    let mut g = Game::new((Class::Shaman, &deck), (Class::Shaman, &deck), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    assert_eq!(g.players[0].hero_power.name(), "Mug's Magic");
}

#[test]
fn mugs_magic_discounts_only_the_first_minion_each_turn_from_turn_three() {
    let yeti = by_name("Chillwind Yeti").unwrap();
    let mut f = Fix::new();
    f.g.players[0].hero_power = by_name("Mug's Magic").unwrap();
    f.g.players[0].hand.push(HandCard::new(yeti));

    f.g.turn = 2;
    assert_eq!(f.g.card_cost(ME, 0), 4, "not yet turn 3");

    f.g.turn = 3;
    assert_eq!(
        f.g.card_cost(ME, 0),
        2,
        "the first minion is discounted from turn 3"
    );

    f.play("Wisp", None); // a different minion, played first, spends the discount
    assert_eq!(
        f.g.card_cost(ME, 0),
        4,
        "the discount is already spent this turn"
    );
}

#[test]
fn zees_might_doubles_the_battlecry_of_every_fifth_minion_played() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.players[0].hero_power = by_name("Zee's Might").unwrap();
    f.g.players[0].minions_played_total = 4;
    f.play("Novice Engineer", None); // battlecry: draw a card
    assert_eq!(
        f.g.players[0].hand.len(),
        2,
        "the 5th minion's battlecry fired twice"
    );
}
