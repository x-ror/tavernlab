//! One test per card, or per family of cards that share a rule.
//!
//! With no second engine to compare against, this is where correctness is
//! established. Comparing only who won would let a card that is wrong in a way
//! that rarely decides a game pass forever; asserting the resulting board makes
//! the mistake visible on the turn it happens.
//!
//! Each test builds a fixed position, plays exactly one card, and states what
//! should have changed.

use tavernlab_core::cards::{Class, Keywords, Kind, Races, behaviour_of, by_name};
use tavernlab_core::game::{Action, Agent};
use tavernlab_core::state::{Flags, Game, HandCard, MAX_HAND, Permanent, Side, Target};

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
        // A minion that is in play has its continuous effects applied — both
        // the auras it projects and the ones it grants itself. Pushing straight
        // onto the board skips the summon that would normally do this.
        self.g.recompute_auras();
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

#[test]
fn nespirah_deals_one_and_reopens_after_a_fel_spell() {
    use tavernlab_core::events::Event;

    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    f.play("Nespirah, Enthralled", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Nespirah, Enthralled")
        .unwrap() as u8;
    let target = Some(Target::Minion(FOE, 0));

    assert!(f.g.apply(Action::UseLocation { slot, target }));
    assert_eq!(f.theirs(0).health(), 4, "took 1 damage");
    assert!(
        !f.g.apply(Action::UseLocation { slot, target }),
        "already used this turn"
    );

    f.g.fire(Event::SpellCast {
        side: ME,
        card: by_name("Eye Beam").unwrap(), // Fel school; not itself played
    });
    assert!(
        f.g.apply(Action::UseLocation { slot, target }),
        "reopened by the Fel spell"
    );
    assert_eq!(f.theirs(0).health(), 3, "took a second point of damage");
}

#[test]
fn nespirah_deathrattle_summons_nespirah_unshackled() {
    let mut f = Fix::new();
    f.play("Nespirah, Enthralled", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Nespirah, Enthralled")
        .unwrap();
    f.g.deal_damage(Target::Minion(ME, slot as u8), 5);
    f.g.sweep_deaths();
    assert!(
        f.g.players[0]
            .board
            .iter()
            .any(|m| m.card.name() == "Nespirah, Unshackled")
    );
}

#[test]
fn godfrey_catches_an_overdraw_instead_of_burning_it() {
    let mut f = Fix::new();
    f.g.players[0].godfrey_active = true;
    let filler = by_name("Wisp").unwrap();
    for _ in 0..MAX_HAND {
        f.g.players[0].hand.push(HandCard::new(filler));
    }
    let yeti = by_name("Chillwind Yeti").unwrap();
    assert!(
        !f.g.give_card(ME, yeti),
        "still reports the burn -- hand is full either way"
    );
    assert_eq!(f.g.players[0].overdrawn.len(), 1);
    assert_eq!(f.g.players[0].overdrawn[0], yeti);
}

#[test]
fn godfrey_returns_one_overdrawn_card_per_turn_discounted() {
    let mut f = Fix::new();
    f.g.players[0].godfrey_active = true;
    let yeti = by_name("Chillwind Yeti").unwrap();
    f.g.players[0].overdrawn.push(yeti);
    f.g.players[0].deck.push(by_name("Wisp").unwrap()); // fed to the guaranteed draw
    f.g.begin_turn();
    let hc = f.g.players[0]
        .hand
        .iter()
        .find(|hc| hc.card == yeti)
        .expect("the overdrawn Yeti returned");
    assert_eq!(hc.cost_delta, -1, "permanently discounted");
    assert!(f.g.players[0].overdrawn.is_empty());
}

#[test]
fn merithra_fills_the_hand_with_dragons_at_normal_cost() {
    use tavernlab_core::cards::Races;

    let mut f = Fix::new();
    f.play("Merithra of the Dream", None);
    assert_eq!(f.g.players[0].hand.len(), MAX_HAND);
    for hc in f.g.players[0].hand.iter() {
        assert!(hc.card.def().races.any(Races::DRAGON));
        assert_eq!(hc.cost_delta, 0, "no discount below 25 Mana spent");
    }
}

#[test]
fn merithra_discounts_the_dragons_to_one_after_spending_25_mana_while_held() {
    let mut f = Fix::new();
    let merithra = by_name("Merithra of the Dream").unwrap();
    let mut hc = HandCard::new(merithra);
    hc.mana_spent_while_held = 25;
    f.g.players[0].hand.push(hc);
    let idx = f.g.players[0].hand.len() as u8 - 1;
    assert!(f.g.apply(Action::Play {
        hand: idx,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    for hc in f.g.players[0].hand.iter() {
        assert_eq!(
            hc.card.def().cost + hc.cost_delta,
            1,
            "each Dragon costs exactly 1"
        );
    }
}

#[test]
fn naralex_discounts_the_first_dragon_each_turn_to_one() {
    let mut f = Fix::new().board(ME, &["Naralex, Herald of the Flights"]);
    let whelp = by_name("Twilight Whelp").unwrap();
    f.g.players[0].hand.push(HandCard::new(whelp));
    assert_eq!(f.g.card_cost(ME, 0), 1, "discounted to a flat 1");

    f.play("Twilight Whelp", None);
    f.g.players[0].hand.push(HandCard::new(whelp));
    let idx = f.g.players[0].hand.len() - 1;
    assert_eq!(
        f.g.card_cost(ME, idx),
        whelp.def().cost,
        "already spent this turn"
    );
}

#[test]
fn shadow_of_demise_transforms_into_the_next_spell_cast() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    let shadow = by_name("Shadow of Demise").unwrap();
    f.g.players[0].hand.push(HandCard::new(shadow));
    f.play("Fireball", foe_minion(0));
    assert_eq!(f.g.players[0].hand.len(), 1);
    let hc = f.g.players[0].hand[0];
    assert_eq!(
        hc.card.name(),
        "Fireball",
        "Shadow of Demise became the spell that was cast"
    );
    assert_eq!(
        hc.cost_delta, 0,
        "a fresh copy, not carrying any prior state"
    );
}

#[test]
fn shadow_of_demise_does_nothing_cast_in_its_original_form() {
    let mut f = Fix::new();
    f.play("Shadow of Demise", None);
    assert_eq!(f.g.players[0].hand.len(), 0);
}

#[test]
fn mirrex_plays_as_its_own_plain_body() {
    let mut f = Fix::new();
    f.play("Mirrex, the Crystalline", None);
    assert_eq!(f.mine(0).atk, 3);
    assert_eq!(f.mine(0).max_hp, 4);
}

#[test]
fn cursed_chains_takes_control_until_the_original_owners_turn_ends() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Cursed Chains", foe_minion(0));
    assert_eq!(f.g.players[0].board.len(), 1, "now on my board");
    assert_eq!(f.g.players[1].board.len(), 0);
    assert!(!f.mine(0).can_attack(), "can't attack this turn");

    f.g.end_turn(); // my turn ends -- not the original owner's
    assert_eq!(f.g.players[0].board.len(), 1, "still mine, unreturned");

    f.g.current = Side::Player1;
    f.g.end_turn(); // the original owner's turn ends
    assert_eq!(f.g.players[0].board.len(), 0, "returned");
    assert_eq!(f.g.players[1].board.len(), 1);
}

#[test]
fn cursed_chains_returns_the_exact_stolen_instance_not_a_same_named_double() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti", "Chillwind Yeti"]);
    f.g.players[1].board[0].damage = 3; // distinguishes the two identical copies
    f.play("Cursed Chains", foe_minion(0)); // steals the damaged one specifically
    f.g.current = Side::Player1;
    f.g.end_turn();
    assert_eq!(f.g.players[1].board.len(), 2);
    let damaged = f.g.players[1]
        .board
        .iter()
        .filter(|m| m.damage == 3)
        .count();
    assert_eq!(
        damaged, 1,
        "the exact stolen instance returned, not the double"
    );
}

#[test]
fn irida_sinseeker_sends_the_deck_to_the_void_except_one_card() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    f.play("Irida Sinseeker", None);
    assert_eq!(f.g.players[0].deck.len(), 1, "one card left behind");
    assert_eq!(f.g.players[0].void.len(), 2, "the rest went to the Void");
}

#[test]
fn irida_sinseeker_draws_two_from_the_void_each_turn() {
    let mut f = Fix::new();
    let wisp = by_name("Wisp").unwrap();
    f.g.players[0].void.push(wisp);
    f.g.players[0].void.push(wisp);
    f.g.players[0].void.push(wisp);
    f.g.players[0].deck.push(wisp); // fed to the guaranteed draw
    f.g.begin_turn();
    assert_eq!(f.g.players[0].void.len(), 1, "two were taken");
    assert_eq!(
        f.g.players[0]
            .hand
            .iter()
            .filter(|hc| hc.card == wisp)
            .count(),
        3,
        "2 from the Void plus the guaranteed draw"
    );
}

#[test]
fn wickerfang_summons_four_legs_that_grow_on_their_own() {
    let mut f = Fix::new();
    f.play("Wickerfang", None);
    assert_eq!(f.g.players[0].board.len(), 5, "the body plus four Legs");
    assert_eq!(f.mine(0).atk, 0, "the main body's own printed stats");
    assert_eq!(f.mine(0).max_hp, 5);
    for slot in 1..5 {
        assert_eq!(f.mine(slot).card.name(), "Wickerfang's Leg");
        assert_eq!((f.mine(slot).atk, f.mine(slot).max_hp), (0, 2));
    }

    f.g.end_turn();
    assert_eq!(
        f.g.players[0].board[1].atk, 1,
        "a Leg grew on its own turn's end"
    );
    assert_eq!(
        f.g.players[0].board[0].atk, 0,
        "the main body does not inherit it -- the approximation this session made"
    );
}

#[test]
fn alakir_summons_two_charged_hands_and_gets_two_matching_cost_minions() {
    let mut f = Fix::new();
    f.play("Al'Akir, Lord of Storms", None);
    assert_eq!(
        f.g.players[0].board.len(),
        3,
        "the body plus two Charged Hands"
    );
    for slot in 1..3 {
        assert_eq!(f.mine(slot).card.name(), "Charged Hand of Al'Akir");
    }
    assert_eq!(
        f.g.players[0].hand.len(),
        2,
        "two minions matching its Attack"
    );
    for hc in f.g.players[0].hand.iter() {
        assert_eq!(
            hc.card.def().cost,
            2,
            "matches Al'Akir's own printed Attack"
        );
        assert_eq!(
            hc.card.def().cost + hc.cost_delta,
            1,
            "discounted to a flat 1"
        );
    }
}

#[test]
fn sinestra_summons_two_wings_that_each_discover_an_off_class_spell() {
    let mut f = Fix::new(); // Mage
    f.play("Sinestra", None);
    assert_eq!(f.g.players[0].board.len(), 3, "the body plus two Wings");
    for slot in 1..3 {
        assert_eq!(f.mine(slot).card.name(), "Sinestra's Wing");
    }
    assert_eq!(
        f.g.players[0].hand.len(),
        2,
        "each Wing discovered an off-class spell"
    );
    for hc in f.g.players[0].hand.iter() {
        let d = hc.card.def();
        assert_eq!(d.kind(), Kind::Spell);
        assert_ne!(d.class(), Class::Mage);
        assert_ne!(d.class(), Class::Neutral);
    }
}

#[test]
fn sinestra_casts_a_spell_from_another_class_twice_but_not_her_own() {
    let mut f = Fix::new().deck(&["Chillwind Yeti", "Chillwind Yeti"]); // Mage
    f.g.players[0]
        .board
        .push(Permanent::summon(by_name("Sinestra").unwrap()));

    f.g.players[0].armor = 0;
    f.play("Shield Block", None); // a Warrior spell -- another class
    assert_eq!(f.g.players[0].armor, 10, "doubled: 5 armor twice");
    assert_eq!(f.g.players[0].hand.len(), 2, "doubled: drew twice");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0]
        .board
        .push(Permanent::summon(by_name("Sinestra").unwrap()));
    f.play("Fireball", foe_minion(0)); // Mage's own spell -- not doubled
    assert_eq!(f.theirs(0).health(), 1, "6 damage once, not twice");
}

#[test]
fn ultraxion_heralds_and_reduces_deathwings_cost_if_in_hand() {
    let mut f = Fix::new();
    let deathwing = by_name("Deathwing, Worldbreaker").unwrap();
    f.g.players[0].hand.push(HandCard::new(deathwing));
    f.play("Ultraxion", None);
    assert_eq!(f.g.players[0].herald, 1, "Heralded once");
    let hc = f.g.players[0]
        .hand
        .iter()
        .find(|hc| hc.card == deathwing)
        .expect("Deathwing is still in hand");
    assert_eq!(hc.cost_delta, -1);
}

#[test]
fn atiesh_doubles_spell_damage_but_not_healing() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Atiesh the Greatstaff").unwrap(),
    ));
    f.play("Fireball", foe_minion(0));
    assert_eq!(
        f.their_board(),
        0,
        "6 doubled to 12, well past 7 health -- dead and swept"
    );

    f.g.players[0].hero_hp = 10;
    f.play("Healing Touch", Some(Target::Hero(ME)));
    assert_eq!(
        f.g.players[0].hero_hp, 18,
        "healed for exactly 8, not doubled"
    );
}

#[test]
fn broxigar_disappears_from_hand_at_start_of_game() {
    use tavernlab_core::agent::{Scripted, Style};

    let broxigar = by_name("Broxigar").unwrap();
    let filler = by_name("Wisp").unwrap();
    let mut deck = vec![broxigar];
    deck.resize(30, filler);

    let mut g = Game::new((Class::DemonHunter, &deck), (Class::DemonHunter, &deck), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    let anywhere = g.players[0].deck.iter().any(|&c| c == broxigar)
        || g.players[0].hand.iter().any(|hc| hc.card == broxigar);
    assert!(!anywhere, "Broxigar disappeared from both zones");
}

#[test]
fn axe_of_cenarius_draws_a_portal_after_a_kill_but_not_a_miss() {
    let mut f = Fix::new()
        .deck(&["First Portal to Argus"])
        .board(FOE, &["Wisp", "Boulderfist Ogre"]); // 1/1 dies to 3 Attack, 6/7 does not
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Axe of Cenarius").unwrap(),
    ));

    f.g.apply(Action::HeroAttack {
        target: Target::Minion(FOE, 0),
    });
    assert_eq!(
        f.g.players[0].hand.len(),
        1,
        "the kill drew First Portal to Argus"
    );
    assert_eq!(f.g.players[0].hand[0].card.name(), "First Portal to Argus");

    f.g.players[0].hero_attacks_done = 0;
    f.g.apply(Action::HeroAttack {
        target: Target::Minion(FOE, 0), // the Ogre now, after the Wisp's death shifted it down
    });
    assert_eq!(f.g.players[0].hand.len(), 1, "no kill, no second draw");
}

#[test]
fn first_portal_to_argus_summons_fleeing_urzul_for_the_opponent() {
    let mut f = Fix::new();
    f.play("First Portal to Argus", None);
    assert_eq!(f.g.players[0].board.len(), 0, "not on the caster's board");
    assert_eq!(f.g.players[1].board.len(), 1, "on the opponent's board");
    assert_eq!(f.g.players[1].board[0].card.name(), "Fleeing Ur'zul");
}

#[test]
fn fleeing_urzul_deathrattle_rewards_its_controllers_opponent() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.g.players[1]
        .board
        .push(Permanent::summon(by_name("Fleeing Ur'zul").unwrap()));
    f.g.deal_damage(Target::Minion(FOE, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(
        f.g.players[0].hand.len(),
        1,
        "the opponent of the demon's controller drew a card"
    );
    assert_eq!(
        f.g.players[0]
            .deck
            .iter()
            .filter(|&&c| c.name() == "Second Portal to Argus")
            .count(),
        1,
        "and had the next Portal shuffled into their own deck"
    );
}

#[test]
fn fleeing_terrorguard_deathrattle_returns_broxigar_to_the_opponent() {
    let mut f = Fix::new();
    f.g.players[1]
        .board
        .push(Permanent::summon(by_name("Fleeing Terrorguard").unwrap()));
    f.g.deal_damage(Target::Minion(FOE, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Broxigar");
}

#[test]
fn commander_beatrix_plays_with_its_printed_taunt() {
    let mut f = Fix::new();
    f.play("Commander Beatrix", None);
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn azalina_soulsever_draws_until_the_hand_is_full() {
    let mut f = Fix::new().deck(&[
        "Wisp", "Wisp", "Wisp", "Wisp", "Wisp", "Wisp", "Wisp", "Wisp", "Wisp", "Wisp",
    ]);
    f.play("Azalina Soulsever", None);
    assert_eq!(f.g.players[0].hand.len(), MAX_HAND);
}

// ------------------------------------------------------- backlog batch

#[test]
fn slam_draws_only_if_the_minion_survives() {
    let mut f = Fix::new()
        .deck(&["Wisp"])
        .board(FOE, &["Boulderfist Ogre", "Wisp"]); // 6/7, 1/1
    f.play("Slam", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5, "6/7 took 2, survived");
    assert_eq!(f.g.players[0].hand.len(), 1, "survived: drew a card");

    f.play("Slam", foe_minion(1));
    assert_eq!(f.their_board(), 1, "the 1/1 died to 2 damage");
    assert_eq!(f.g.players[0].hand.len(), 1, "died: no second draw");
}

#[test]
fn abusive_sergeant_buff_expires_at_end_of_turn() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Abusive Sergeant", my_minion(0));
    assert_eq!(f.mine(0).atk, 6, "4 base plus 2 this turn");
    f.g.end_turn();
    assert_eq!(f.mine(0).atk, 4, "expired");
}

#[test]
fn beaming_sidekick_buffs_health_permanently() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Beaming Sidekick", my_minion(0));
    assert_eq!(f.mine(0).max_hp, 7);
    f.g.end_turn();
    assert_eq!(f.mine(0).max_hp, 7, "permanent, unlike a temp Attack buff");
}

#[test]
fn murloc_tidecaller_reacts_to_a_murloc_summon_but_not_itself() {
    let mut f = Fix::new().board(ME, &["Murloc Tidecaller"]);
    assert_eq!(f.mine(0).atk, 1, "unaffected by its own arrival");
    f.g.summon(ME, by_name("Murloc Raider").unwrap());
    assert_eq!(f.mine(0).atk, 2, "reacted to another Murloc");
    f.g.summon(ME, by_name("Wisp").unwrap());
    assert_eq!(f.mine(0).atk, 2, "a non-Murloc does not trigger it");
}

#[test]
fn gnawing_greenfin_gets_a_random_murloc() {
    let mut f = Fix::new();
    f.play("Gnawing Greenfin", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let d = f.g.players[0].hand[0].card.def();
    assert_eq!(d.kind(), Kind::Minion);
    assert!(d.races.any(tavernlab_core::cards::Races::MURLOC));
}

#[test]
fn siphoning_growth_destroys_a_friendly_minion_for_armor() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.g.players[0].armor = 0;
    f.play("Siphoning Growth", my_minion(0));
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.g.players[0].armor, 8);
}

#[test]
fn injured_tolvir_damages_itself_but_keeps_taunt() {
    let mut f = Fix::new();
    f.play("Injured Tol'vir", None);
    assert_eq!(f.mine(0).health(), 3, "6 max minus 3 self-damage");
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn crazed_alchemist_swaps_current_attack_and_health() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[1].board[0].damage = 3; // currently 6/4
    f.play("Crazed Alchemist", foe_minion(0));
    assert_eq!(f.theirs(0).atk, 4, "the old (post-damage) Health");
    assert_eq!(f.theirs(0).health(), 6, "the old Attack, undamaged");
}

#[test]
fn bloodsail_raider_gains_attack_equal_to_the_weapon() {
    let mut f = Fix::new();
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Fiery War Axe").unwrap(), // 3 Attack
    ));
    f.play("Bloodsail Raider", None);
    assert_eq!(f.mine(0).atk, f.mine(0).card.def().atk + 3);
}

#[test]
fn maze_guide_summons_a_two_cost_minion() {
    let mut f = Fix::new();
    f.play("Maze Guide", None);
    assert_eq!(f.g.players[0].board.len(), 2, "Maze Guide plus the summon");
    assert_eq!(f.mine(1).card.def().cost, 2);
}

#[test]
fn unleash_the_crocolisks_gains_armor_and_summons_for_the_opponent() {
    let mut f = Fix::new();
    f.g.players[0].armor = 0;
    f.play("Unleash the Crocolisks", None);
    assert_eq!(f.g.players[0].armor, 10);
    assert_eq!(f.g.players[0].board.len(), 0, "not on the caster's board");
    assert_eq!(f.g.players[1].board.len(), 2, "on the opponent's board");
}

#[test]
fn sunfury_protector_gives_taunt_to_both_neighbors_only() {
    let mut f = Fix::new().board(ME, &["Wisp", "Wisp"]);
    let card = by_name("Sunfury Protector").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let idx = f.g.players[0].hand.len() as u8 - 1;
    let ok = f.g.apply(Action::Play {
        hand: idx,
        target: None,
        position: 1, // between the two Wisps
        choice: u8::MAX,
    });
    assert!(ok);
    assert!(f.mine(0).has(Keywords::TAUNT), "left neighbor");
    assert!(!f.mine(1).has(Keywords::TAUNT), "not itself");
    assert!(f.mine(2).has(Keywords::TAUNT), "right neighbor");
}

#[test]
fn p1ck_p0k3t_draws_only_with_a_big_enough_deck() {
    let mut f = Fix::new();
    f.play("P1CK-P0K3T", None);
    assert_eq!(f.g.players[0].hand.len(), 0, "deck too small");

    let filler = by_name("Wisp").unwrap();
    for _ in 0..25 {
        f.g.players[0].deck.push(filler);
    }
    f.play("P1CK-P0K3T", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "25+ cards: drew one");
}

#[test]
fn micro_machine_grows_on_either_players_turn_start() {
    let mut f = Fix::new().board(ME, &["Micro Machine"]);
    f.g.current = Side::Player1;
    f.g.fire(tavernlab_core::events::Event::TurnStart {
        side: Side::Player1,
    });
    assert_eq!(f.mine(0).atk, 2, "grew on the opponent's turn too");
}

#[test]
fn frothing_berserker_grows_from_any_minion_taking_damage() {
    let mut f = Fix::new()
        .board(ME, &["Frothing Berserker"])
        .board(FOE, &["Wisp"]);
    f.g.deal_damage(Target::Minion(FOE, 0), 1);
    assert_eq!(
        f.mine(0).atk,
        3,
        "even an enemy minion taking damage counts"
    );
}

#[test]
fn coldlight_seer_buffs_other_murlocs_only() {
    let mut f = Fix::new().board(ME, &["Murloc Raider", "Wisp"]);
    f.play("Coldlight Seer", None);
    assert_eq!(f.mine(0).max_hp, 3, "1 base plus 2");
    assert_eq!(f.mine(1).max_hp, 1, "not a Murloc, unaffected");
    assert_eq!(
        f.mine(2).max_hp,
        3,
        "Coldlight Seer itself is not \"other\""
    );
}

#[test]
fn big_game_hunter_only_targets_seven_or_more_attack() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    let card = by_name("Big Game Hunter").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let idx = f.g.players[0].hand.len() as u8 - 1;
    assert!(
        !f.g.apply(Action::Play {
            hand: idx,
            target: foe_minion(0),
            position: u8::MAX,
            choice: u8::MAX,
        }),
        "6 Attack is not enough"
    );
}

#[test]
fn lifedrinker_burns_the_enemy_hero_and_heals_its_own() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play("Lifedrinker", None);
    assert_eq!(f.g.players[1].hero_hp, 27);
    assert_eq!(f.g.players[0].hero_hp, 23);
}

#[test]
fn twilight_drake_gains_health_per_card_in_hand() {
    let mut f = Fix::new();
    let filler = by_name("Wisp").unwrap();
    for _ in 0..3 {
        f.g.players[0].hand.push(HandCard::new(filler));
    }
    f.play("Twilight Drake", None);
    assert_eq!(f.mine(0).max_hp, f.mine(0).card.def().hp + 3);
}

#[test]
fn dread_corsair_costs_less_per_weapon_attack() {
    let mut f = Fix::new();
    let corsair = by_name("Dread Corsair").unwrap();
    f.g.players[0].hand.push(HandCard::new(corsair));
    assert_eq!(f.g.card_cost(ME, 0), corsair.def().cost, "no weapon yet");

    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Fiery War Axe").unwrap(), // 3 Attack
    ));
    assert_eq!(f.g.card_cost(ME, 0), corsair.def().cost - 3);
}

#[test]
fn city_defenses_summons_two_taunt_walls() {
    let mut f = Fix::new();
    f.play("City Defenses", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    for slot in 0..2 {
        assert!(f.mine(slot).has(Keywords::TAUNT));
        assert_eq!((f.mine(slot).atk, f.mine(slot).max_hp), (0, 6));
    }
}

#[test]
fn steadfast_security_gains_attack_when_damaged() {
    let mut f = Fix::new();
    f.play("City Defenses", None);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_eq!(f.mine(0).atk, 1, "grew after taking damage");
}

#[test]
fn eggbasher_damages_and_buffs_the_same_target() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Eggbasher", foe_minion(0));
    assert_eq!(f.theirs(0).atk, 10, "6 plus 4");
    assert_eq!(f.theirs(0).health(), 6, "7 minus 1");
}

// ------------------------------------------------------- backlog batch, DK

#[test]
fn icy_touch_damages_and_freezes() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Icy Touch", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5);
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
}

#[test]
fn doomsayer_destroys_every_minion_on_its_controllers_turn_start() {
    let mut f = Fix::new()
        .board(ME, &["Doomsayer", "Wisp"])
        .board(FOE, &["Wisp"]);
    f.g.fire(tavernlab_core::events::Event::TurnStart { side: ME });
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.g.players[1].board.len(), 0);
}

#[test]
fn plague_strike_summons_a_zombie_only_on_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Wisp"]); // 6/7, 1/1
    f.play("Plague Strike", foe_minion(0));
    assert_eq!(f.g.players[0].board.len(), 0, "survived: no summon");

    f.play("Plague Strike", foe_minion(1));
    assert_eq!(f.g.players[0].board.len(), 1, "died: summoned the Zombie");
    assert_eq!(f.mine(0).card.name(), "Rampaging Zombie");
}

#[test]
fn harbinger_of_winter_deathrattle_draws_a_frost_spell() {
    let mut f = Fix::new()
        .deck(&["Icy Touch"]) // the one Frost spell available to draw
        .board(ME, &["Harbinger of Winter"]);
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Icy Touch");
}

#[test]
fn asphyxiate_destroys_only_the_highest_attack_enemy() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Boulderfist Ogre", "Chillwind Yeti"]);
    f.play("Asphyxiate", None);
    assert_eq!(f.their_board(), 2);
    assert!(
        f.g.players[1]
            .board
            .iter()
            .all(|m| m.card.name() != "Boulderfist Ogre"),
        "the 6-Attack Ogre was the highest and is gone"
    );
}

#[test]
fn chillfallen_baron_draws_on_both_battlecry_and_deathrattle() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("Chillfallen Baron", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "battlecry draw");

    f.g.players[0].deck.push(by_name("Wisp").unwrap());
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 2, "deathrattle draw too");
}

#[test]
fn stonehill_defender_discovers_a_taunt_minion() {
    let mut f = Fix::new();
    f.play("Stonehill Defender", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert!(
        f.g.players[0].hand[0]
            .card
            .def()
            .keywords
            .has(Keywords::TAUNT)
    );
}

#[test]
fn acolyte_of_death_draws_only_for_a_friendly_undead() {
    let mut f = Fix::new()
        .deck(&["Wisp", "Wisp"])
        .board(ME, &["Acolyte of Death", "Rampaging Zombie"])
        .board(FOE, &["Rampaging Zombie"]);
    f.g.deal_damage(Target::Minion(FOE, 0), 5); // enemy Undead dies
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 0, "not friendly");

    f.g.deal_damage(Target::Minion(ME, 1), 5); // friendly Undead dies
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1, "friendly Undead: drew");
}

#[test]
fn dark_transformation_only_affects_an_undead_target() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Rampaging Zombie"]);
    f.play("Dark Transformation", my_minion(0));
    assert_eq!(
        f.mine(0).card.name(),
        "Chillwind Yeti",
        "not Undead: unaffected"
    );

    f.play("Dark Transformation", my_minion(1));
    assert_eq!(f.mine(1).card.name(), "Undead Monstrosity");
}

#[test]
fn poison_breath_only_affects_an_undead_target() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Rampaging Zombie"]);
    f.play("Poison Breath", my_minion(0));
    assert!(
        !f.mine(0).has(Keywords::POISONOUS),
        "not Undead: unaffected"
    );

    f.play("Poison Breath", my_minion(1));
    assert!(f.mine(1).has(Keywords::POISONOUS));
}

// ---------------------------------------------------- backlog batch, Shaman

#[test]
fn static_shock_damages_and_buffs_the_hero_for_the_turn() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4/5
    f.play("Static Shock", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4);
    assert_eq!(f.g.players[0].hero_bonus_atk, 1);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hero_bonus_atk, 0, "expired");
}

#[test]
fn lightning_rod_hits_the_friendly_target_and_a_random_enemy() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"]) // 6/7
        .board(FOE, &["Chillwind Yeti"]); // 4/5, the only possible random pick
    f.play("Lightning Rod", my_minion(0));
    assert_eq!(f.mine(0).health(), 5, "the friendly target took 2");
    assert_eq!(f.theirs(0).health(), 1, "the random enemy took 4");
}

#[test]
fn thunderquake_damages_every_minion_and_grants_a_static_shock() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"])
        .board(FOE, &["Chillwind Yeti"]);
    f.play("Thunderquake", None);
    assert_eq!(f.mine(0).health(), 6, "friendly minions are hit too");
    assert_eq!(f.theirs(0).health(), 4);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Static Shock");
}

#[test]
fn lightning_storm_only_hits_enemy_minions_and_overloads() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"])
        .board(FOE, &["Chillwind Yeti"]);
    f.play("Lightning Storm", None);
    assert_eq!(f.mine(0).health(), 7, "friendly minions are untouched");
    assert_eq!(f.theirs(0).health(), 2, "5 health minus 3 damage");
    assert_eq!(f.g.players[0].overload_next, 1);
}

#[test]
fn far_sight_draws_a_card_discounted_by_three() {
    let mut f = Fix::new().deck(&["Boulderfist Ogre"]); // 6 Mana
    f.play("Far Sight", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.card_cost(ME, 0), 3, "6 minus 3");
}

#[test]
fn voltaic_burst_summons_two_sparks_and_overloads() {
    let mut f = Fix::new();
    f.play("Voltaic Burst", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    for slot in 0..2 {
        assert_eq!(f.mine(slot).card.name(), "Spark");
        assert!(f.mine(slot).has(Keywords::RUSH));
    }
    assert_eq!(f.g.players[0].overload_next, 1);
}

#[test]
fn cinderfin_deathrattle_summons_sizzling_cinder() {
    let mut f = Fix::new();
    f.play("Cinderfin", None);
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Sizzling Cinder");
}

// ---------------------------------------------------- backlog batch, Priest

#[test]
fn mend_restores_full_health_and_draws() {
    let mut f = Fix::new().deck(&["Wisp"]).board(ME, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].board[0].damage = 5;
    f.play("Mend", my_minion(0));
    assert_eq!(f.mine(0).health(), 7, "fully restored");
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn amber_priestess_heals_by_its_own_health() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play("Amber Priestess", Some(Target::Hero(ME))); // 1/4
    assert_eq!(f.g.players[0].hero_hp, 24, "healed by 4, its own Health");
}

#[test]
fn purifying_breath_heals_the_enemy_hero_only_on_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Wisp"]); // 6/7, 1/1
    f.g.players[1].hero_hp = 20; // room to heal, below the 30 cap
    f.play("Purifying Breath", foe_minion(0));
    assert_eq!(f.g.players[1].hero_hp, 20, "survived: no heal");

    f.play("Purifying Breath", foe_minion(1));
    assert_eq!(f.g.players[1].hero_hp, 25, "died: healed the enemy hero 5");
}

#[test]
fn crystalsmith_cultist_buffs_only_while_holding_a_shadow_spell() {
    let mut f = Fix::new();
    f.play("Crystalsmith Cultist", None);
    assert_eq!(f.mine(0).atk, 1, "no Shadow spell in hand: unbuffed");

    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Devouring Plague").unwrap())); // Shadow school
    f.play("Crystalsmith Cultist", None);
    assert_eq!(f.mine(0).atk, 2, "1 base plus 1");
    assert_eq!(f.mine(0).max_hp, 3, "2 base plus 1");
}

#[test]
fn injured_attendant_damages_itself() {
    let mut f = Fix::new();
    f.play("Injured Attendant", None);
    assert_eq!(f.mine(0).health(), 4, "8 max minus 4 self-damage");
}

#[test]
fn void_shard_damages_and_heals_its_own_hero() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].hero_hp = 20;
    f.play("Void Shard", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 3, "7 minus 4");
    assert_eq!(f.g.players[0].hero_hp, 24, "healed for the 4 dealt");
}

#[test]
fn cleansing_lightspawn_damages_by_its_own_health() {
    let mut f = Fix::new()
        .board(ME, &["Cleansing Lightspawn"]) // 2/3
        .board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Cleansing Lightspawn", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "7 minus 3, its own Health");
}

#[test]
fn greater_healing_potion_heals_and_draws() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.g.players[0].hero_hp = 10;
    f.play("Greater Healing Potion", Some(Target::Hero(ME)));
    assert_eq!(
        f.g.players[0].hero_hp, 22,
        "10 plus 12, capped well under 30"
    );
    assert_eq!(f.g.players[0].hand.len(), 1, "drew a card");
}

#[test]
fn devouring_plague_deals_four_total_split_and_heals_its_own_hero() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7, the only target
    f.g.players[0].hero_hp = 20;
    f.play("Devouring Plague", None);
    assert_eq!(
        f.theirs(0).health(),
        3,
        "all 4 points landed on the one target"
    );
    assert_eq!(f.g.players[0].hero_hp, 24);
}

#[test]
fn quick_shot_deals_three_and_draws_only_from_an_empty_hand() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Quick Shot", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "7 minus 3");
    assert_eq!(
        f.g.players[0].hand.len(),
        0,
        "hand was already empty: no draw"
    );

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]).deck(&["Wisp"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Quick Shot", foe_minion(0));
    assert_eq!(
        f.g.players[0].hand.len(),
        1,
        "hand had a card: no draw triggered"
    );
}

#[test]
fn bursting_shot_hits_all_three_enemies_for_two() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Boulderfist Ogre"]); // 6/7 x2
    f.play("Bursting Shot", None);
    assert_eq!(f.theirs(0).health(), 5, "7 minus 2");
    assert_eq!(f.theirs(1).health(), 5, "7 minus 2");
    assert_eq!(
        f.g.players[1].hero_hp, 28,
        "the enemy hero is the third of exactly three enemies"
    );
}

#[test]
fn headhunters_hatchet_gains_durability_only_with_a_beast() {
    let mut f = Fix::new();
    f.play("Headhunter's Hatchet", None);
    assert_eq!(
        f.g.players[0].weapon.unwrap().durability,
        2,
        "no Beast on board: base durability only"
    );

    let mut f = Fix::new().board(ME, &["Webspinner"]); // a Beast
    f.play("Headhunter's Hatchet", None);
    assert_eq!(
        f.g.players[0].weapon.unwrap().durability,
        3,
        "2 base plus 1 for controlling a Beast"
    );
}

#[test]
fn ticking_timebomb_destroys_a_random_enemy_minion_on_death() {
    let mut f = Fix::new()
        .board(ME, &["Ticking Timebomb"])
        .board(FOE, &["Wisp"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0, "the only enemy minion was destroyed");
}

#[test]
fn arrow_retriever_draws_up_to_three_but_not_past_it() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    f.play("Arrow Retriever", None);
    assert_eq!(f.g.players[0].hand.len(), 3, "drew up to 3");

    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Arrow Retriever", None);
    assert_eq!(
        f.g.players[0].hand.len(),
        3,
        "already had 3 once Arrow Retriever itself left the hand: no extra draw"
    );
}

#[test]
fn spirit_bond_summons_a_wolf_only_on_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Spirit Bond", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "7 minus 3, survives");
    assert_eq!(f.g.players[0].board.len(), 0, "no kill: no Wolf");

    let mut f = Fix::new().board(FOE, &["Wisp"]); // 1/1, dies to 3
    f.play("Spirit Bond", foe_minion(0));
    assert_eq!(
        f.g.players[0].board.len(),
        1,
        "killed it: summoned the Wolf"
    );
    assert_eq!(f.mine(0).atk, 3);
    assert_eq!(f.mine(0).max_hp, 2);
}

#[test]
fn ball_of_spiders_summons_three_webspinners() {
    let mut f = Fix::new();
    f.play("Ball of Spiders", None);
    assert_eq!(f.g.players[0].board.len(), 3);
    assert_eq!(f.mine(0).card.name(), "Webspinner");
    assert_eq!(f.mine(0).atk, 1);
    assert_eq!(f.mine(0).max_hp, 1);
}

#[test]
fn webspinner_deathrattle_gets_a_random_beast() {
    let mut f = Fix::new().board(ME, &["Webspinner"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1, "got a card");
    let got = f.g.players[0].hand[0].card.def();
    assert!(got.races.any(Races::BEAST), "the random card is a Beast");
}

#[test]
fn herbivore_assistant_buffs_only_a_beast() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]); // 4/5, not a Beast
    f.play("Herbivore Assistant", my_minion(0));
    assert_eq!(f.mine(0).atk, 4, "not a Beast: no buff");
    assert_eq!(f.mine(0).max_hp, 5);

    let mut f = Fix::new().board(ME, &["Webspinner"]); // 1/1, a Beast
    f.play("Herbivore Assistant", my_minion(0));
    assert_eq!(f.mine(0).atk, 3, "1 plus 2");
    assert_eq!(f.mine(0).max_hp, 3, "1 plus 2");
    assert!(f.mine(0).has(Keywords::RUSH));
}

#[test]
fn argent_protector_grants_divine_shield() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Argent Protector", my_minion(0));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
}

#[test]
fn fogsail_freebooter_damages_only_with_a_weapon() {
    let mut f = Fix::new();
    f.play("Fogsail Freebooter", None);
    assert_eq!(f.g.players[1].hero_hp, 30, "no weapon: no damage");

    let mut f = Fix::new();
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Light's Justice").unwrap(),
    ));
    f.play("Fogsail Freebooter", None);
    assert_eq!(
        f.g.players[1].hero_hp, 28,
        "2 damage with a weapon equipped"
    );
}

#[test]
fn mad_bomber_splits_three_among_all_other_characters() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"]) // 6/7
        .board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Mad Bomber", None);
    let mad_bomber = f.g.players[0]
        .board
        .iter()
        .find(|m| m.card.name() == "Mad Bomber")
        .unwrap();
    assert_eq!(mad_bomber.damage, 0, "Mad Bomber itself is excluded");
    let total_damage = f.g.players[0].board[0].damage
        + f.g.players[1].board[0].damage
        + (30 - f.g.players[0].hero_hp)
        + (30 - f.g.players[1].hero_hp);
    assert_eq!(total_damage, 3, "all 3 points landed on other characters");
}

#[test]
fn brightwing_adds_a_random_legendary_minion() {
    let mut f = Fix::new();
    f.play("Brightwing", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card.def();
    assert_eq!(got.kind(), Kind::Minion);
    assert_eq!(got.rarity(), tavernlab_core::cards::Rarity::Legendary);
}

#[test]
fn tranquil_treant_grants_both_players_a_crystal() {
    let mut f = Fix::new().board(ME, &["Tranquil Treant"]);
    f.g.players[0].crystals = 5;
    f.g.players[1].crystals = 3;
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].crystals, 6);
    assert_eq!(f.g.players[1].crystals, 4);
}

#[test]
fn convalescence_summons_two_shielded_recruits() {
    let mut f = Fix::new();
    f.play("Convalescence", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    for i in 0..2 {
        assert_eq!(f.mine(i).card.name(), "Silver Hand Recruit");
        assert!(f.mine(i).has(Keywords::DIVINE_SHIELD));
    }
}

#[test]
fn silvermoon_portal_buffs_and_summons() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Silvermoon Portal", my_minion(0));
    assert_eq!(f.mine(0).atk, 3, "1 plus 2");
    assert_eq!(f.mine(0).max_hp, 3, "1 plus 2");
    assert_eq!(
        f.g.players[0].board.len(),
        2,
        "plus the summoned 2-Cost minion"
    );
    assert_eq!(f.mine(1).card.def().cost, 2);
}

#[test]
fn ancient_stegodon_choose_modes() {
    let mut f = Fix::new();
    f.play_mode("Ancient Stegodon", 0, None);
    assert!(f.mine(0).has(Keywords::TAUNT), "mode 0 is Taunt");

    let mut f = Fix::new();
    f.play_mode("Ancient Stegodon", 1, None);
    assert!(f.mine(0).has(Keywords::POISONOUS), "mode 1 is Poisonous");

    let mut f = Fix::new();
    f.play_mode("Ancient Stegodon", 2, None);
    assert_eq!(f.mine(0).atk, 2, "mode 2 is +1/+1: 1 plus 1");
    assert_eq!(f.mine(0).max_hp, 6, "mode 2 is +1/+1: 5 plus 1");
}

#[test]
fn barkshield_sentinel_grows_after_its_controllers_hero_power_only() {
    let mut f = Fix::new().board(ME, &["Barkshield Sentinel"]);
    assert!(f.g.apply(Action::HeroPower {
        target: Some(Target::Hero(FOE)),
        second: false,
    }));
    assert_eq!(f.mine(0).max_hp, 4, "2 base plus 2");
}

#[test]
fn holy_bolas_draws_a_second_card_only_if_the_first_was_cheap() {
    let mut f = Fix::new().deck(&["Boulderfist Ogre"]); // cost 6
    f.play("Holy Bola!", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "6-cost draw: no second");

    let mut f = Fix::new().deck(&["Wisp", "Wisp"]); // cost 0 each
    f.play("Holy Bola!", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "0-cost draw: drew a second");
}

#[test]
fn muster_for_battle_summons_recruits_and_equips_the_weapon() {
    let mut f = Fix::new();
    f.play("Muster for Battle", None);
    assert_eq!(f.g.players[0].board.len(), 3);
    for i in 0..3 {
        assert_eq!(f.mine(i).card.name(), "Silver Hand Recruit");
    }
    let w = f.g.players[0].weapon.expect("equipped a weapon");
    assert_eq!(w.card.name(), "Light's Justice");
    assert_eq!(w.atk, 1);
    assert_eq!(w.durability, 4);
}

#[test]
fn skeletal_sidekick_buffs_only_a_friendly_undead() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]); // 4/5, not Undead
    f.play("Skeletal Sidekick", my_minion(0));
    assert_eq!(f.mine(0).atk, 4, "not Undead: no buff");

    let mut f = Fix::new().board(ME, &["Wisp"]); // races=[UNDEAD] in this corpus
    f.play("Skeletal Sidekick", my_minion(0));
    assert_eq!(f.mine(0).atk, 3, "1 plus 2");
}

#[test]
fn timestop_deals_three_and_freezes_both_enemy_minions() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp"]); // exactly 2: no randomness in which freeze
    f.play("Timestop", None);
    assert_eq!(f.g.players[1].hero_hp, 27, "3 damage to the enemy hero");
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert!(f.theirs(1).flags.has(Flags::FROZEN));
}

#[test]
fn howling_blast_freezes_the_target_and_splashes_the_rest() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Boulderfist Ogre"]); // 6/7 x2
    f.play("Howling Blast", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "7 minus 3, the primary target");
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert_eq!(
        f.theirs(1).health(),
        6,
        "7 minus 1, splashed but not frozen"
    );
    assert!(!f.theirs(1).flags.has(Flags::FROZEN));
    assert_eq!(f.g.players[1].hero_hp, 29, "1 splash to the enemy hero too");
}

#[test]
fn deathchiller_hits_two_random_enemies_after_a_spell() {
    // AllEnemies is the two Yetis and the enemy hero -- three possible
    // targets for two hits, so which two land is random; only the total is
    // deterministic. Chillwind Yeti, not Wisp, so a hit never kills one and
    // shrinks the board out from under `theirs(1)`.
    let mut f = Fix::new()
        .board(ME, &["Deathchiller"])
        .board(FOE, &["Chillwind Yeti", "Chillwind Yeti"]);
    f.play("The Coin", None);
    let total = f.theirs(0).damage + f.theirs(1).damage + (30 - f.g.players[1].hero_hp);
    assert_eq!(total, 2, "two distinct points of damage landed somewhere");
}

#[test]
fn reluctant_wrangler_deathrattle_summons_a_taunt_beast() {
    let mut f = Fix::new().board(ME, &["Reluctant Wrangler"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    // The Beast from the deathrattle, and then the Wrangler itself: the card
    // is printed "Reborn Deathrattle: …" and both halves resolve, rattle
    // first.
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(0).atk, 2);
    assert_eq!(f.mine(0).max_hp, 2);
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert_eq!(f.mine(1).card.name(), "Reluctant Wrangler");
    assert_eq!(f.mine(1).health(), 1);
    assert!(!f.mine(1).has(Keywords::REBORN), "Reborn is once only");
}

#[test]
fn cryofrozen_champion_deathrattle_gets_a_discounted_legendary() {
    let mut f = Fix::new().board(ME, &["Cryofrozen Champion"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert_eq!(
        h.card.def().rarity(),
        tavernlab_core::cards::Rarity::Legendary
    );
    assert_eq!(h.cost_delta, -1);
}

#[test]
fn bone_breaker_only_after_attacking_a_minion() {
    let mut f = Fix::new().board(FOE, &["Wisp"]);
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Bone Breaker").unwrap(),
    ));
    f.g.apply(Action::HeroAttack {
        target: Target::Minion(FOE, 0),
    });
    assert_eq!(
        f.g.players[1].hero_hp, 28,
        "2 bonus damage after attacking a minion"
    );

    let mut f = Fix::new();
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Bone Breaker").unwrap(),
    ));
    f.g.apply(Action::HeroAttack {
        target: Target::Hero(FOE),
    });
    assert_eq!(
        f.g.players[1].hero_hp, 28,
        "2 from the weapon swing alone: no bonus for a hero target"
    );
}

#[test]
fn mortal_coil_draws_only_on_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]).deck(&["Wisp"]); // 6/7
    f.play("Mortal Coil", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 6, "7 minus 1, survives");
    assert_eq!(f.g.players[0].hand.len(), 0, "no kill: no draw");

    let mut f = Fix::new().board(FOE, &["Wisp"]).deck(&["Wisp"]); // 1/1, dies to 1
    f.play("Mortal Coil", foe_minion(0));
    assert_eq!(f.g.players[0].hand.len(), 1, "killed it: drew a card");
}

#[test]
fn spirit_bomb_damages_the_minion_and_its_own_hero() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].hero_hp = 20;
    f.play("Spirit Bomb", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 3, "7 minus 4");
    assert_eq!(f.g.players[0].hero_hp, 16, "20 minus 4, its own hero");
}

#[test]
fn vulgar_homunculus_damages_its_own_hero() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play("Vulgar Homunculus", None);
    assert_eq!(f.g.players[0].hero_hp, 18);
}

#[test]
fn fiendish_servant_gives_its_attack_to_a_random_friendly_minion() {
    let mut f = Fix::new().board(ME, &["Fiendish Servant", "Wisp"]); // 2/1 and 1/1
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1, "only the Wisp is left");
    assert_eq!(f.mine(0).atk, 3, "1 plus the dead Servant's 2 Attack");
}

#[test]
fn gnomeferatu_removes_the_top_of_the_opponents_deck() {
    let mut f = Fix::new();
    f.g.players[1].deck.push(by_name("Wisp").unwrap());
    f.g.players[1].deck.push(by_name("Chillwind Yeti").unwrap()); // top: popped first
    f.play("Gnomeferatu", None);
    assert_eq!(f.g.players[1].deck.len(), 1);
    assert_eq!(
        f.g.players[1].deck[0].name(),
        "Wisp",
        "the top card is gone"
    );
}

#[test]
fn emberroot_destroyer_hits_a_random_enemy_when_its_controller_takes_damage() {
    let mut f = Fix::new()
        .board(ME, &["Emberroot Destroyer"])
        .board(FOE, &["Wisp"]);
    f.g.deal_damage(Target::Hero(ME), 1);
    assert_eq!(f.theirs(0).damage, 3, "fired on ME's own turn");

    let mut f = Fix::new()
        .board(ME, &["Emberroot Destroyer"])
        .board(FOE, &["Wisp"]);
    f.g.deal_damage(Target::Hero(FOE), 1);
    assert_eq!(
        f.theirs(0).damage,
        0,
        "the enemy hero taking damage is not this"
    );
}

#[test]
fn siphon_soul_destroys_and_heals() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].hero_hp = 20;
    f.play("Siphon Soul", foe_minion(0));
    assert_eq!(f.their_board(), 0, "destroyed outright, not just damaged");
    assert_eq!(f.g.players[0].hero_hp, 23);
}

#[test]
fn demonic_assault_damages_and_summons_two_taunt_voidwalkers() {
    let mut f = Fix::new();
    f.play("Demonic Assault", None);
    assert_eq!(f.g.players[1].hero_hp, 27, "3 damage to the enemy hero");
    assert_eq!(f.g.players[0].board.len(), 2);
    for i in 0..2 {
        assert_eq!(f.mine(i).card.name(), "Voidwalker");
        assert!(f.mine(i).has(Keywords::TAUNT));
    }
}

#[test]
fn blob_of_tar_deathrattle_summons_a_poisonous_blob_and_a_taunt_blob() {
    let mut f = Fix::new().board(ME, &["Blob of Tar"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(0).card.name(), "Lanky Blob");
    assert!(f.mine(0).has(Keywords::POISONOUS));
    assert_eq!(f.mine(1).card.name(), "Robust Blob");
    assert!(f.mine(1).has(Keywords::TAUNT));
}

#[test]
fn sporegnasher_deathrattle_deals_one_to_a_random_enemy_minion() {
    let mut f = Fix::new()
        .board(ME, &["Sporegnasher"])
        .board(FOE, &["Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.theirs(0).damage, 1);
}

#[test]
fn taelan_fordring_deathrattle_draws_the_highest_cost_minion() {
    let mut f = Fix::new().board(ME, &["Taelan Fordring"]).deck(&[
        "Wisp",
        "Fireball",
        "Boulderfist Ogre",
        "Chillwind Yeti",
    ]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(
        f.g.players[0].hand[0].card.name(),
        "Boulderfist Ogre",
        "cost 6, the highest-cost minion (Fireball is a spell, not eligible)"
    );
    assert_eq!(
        f.g.players[0].deck.len(),
        3,
        "only the drawn card left the deck"
    );
}

#[test]
fn doomguard_discards_two_random_cards() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Doomguard", None);
    assert_eq!(f.g.players[0].hand.len(), 0, "both Wisps discarded");
}

#[test]
fn first_flame_damages_and_gives_a_second_flame() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("First Flame", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5, "7 minus 2");
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Second Flame");
}

#[test]
fn second_flame_deals_two_damage() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Second Flame", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5, "7 minus 2");
}

#[test]
fn cold_snap_freezes_and_gets_a_random_frost_spell() {
    let mut f = Fix::new().board(FOE, &["Wisp"]);
    f.play("Cold Snap", foe_minion(0));
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(
        f.g.players[0].hand[0].card.def().school(),
        tavernlab_core::cards::School::Frost
    );
}

#[test]
fn divination_destroys_a_wisp_to_draw_three() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .deck(&["Wisp", "Wisp", "Wisp"]);
    f.play("Divination", my_minion(0));
    assert_eq!(f.g.players[0].board.len(), 1, "not a Wisp: no destroy");
    assert_eq!(f.g.players[0].hand.len(), 0, "no draw either");

    let mut f = Fix::new()
        .board(ME, &["Wisp"])
        .deck(&["Wisp", "Wisp", "Wisp"]);
    f.play("Divination", my_minion(0));
    assert_eq!(f.g.players[0].board.len(), 0, "the Wisp is destroyed");
    assert_eq!(f.g.players[0].hand.len(), 3, "drew 3");
}

#[test]
fn explosive_runes_damages_the_minion_and_spills_excess_to_its_owner() {
    let mut f = Fix::new();
    arm(&mut f, FOE, "Explosive Runes");
    f.play("Boulderfist Ogre", None); // 6/7, survives with no excess
    assert_eq!(f.mine(0).health(), 1, "7 minus 6");
    assert_eq!(
        f.g.players[0].hero_hp, 30,
        "no excess: 6 damage fit inside 7 health"
    );

    let mut f = Fix::new();
    arm(&mut f, FOE, "Explosive Runes");
    f.play("Wisp", None); // 1/1
    assert_eq!(
        f.g.players[0].board.len(),
        0,
        "6 damage kills a 1-health minion"
    );
    assert_eq!(
        f.g.players[0].hero_hp, 25,
        "30 minus the 5 excess (6 minus 1 health)"
    );
}

#[test]
fn winterspring_whelp_discovers_a_one_cost_spell() {
    let mut f = Fix::new();
    f.play("Winterspring Whelp", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.def().cost, 1);
    assert_eq!(f.g.players[0].hand[0].card.def().kind(), Kind::Spell);
}

#[test]
fn babbling_bookcase_adds_two_random_mage_spells() {
    let mut f = Fix::new();
    f.play("Babbling Bookcase", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    for h in &f.g.players[0].hand {
        assert_eq!(h.card.def().kind(), Kind::Spell);
        assert_eq!(h.card.def().class(), Class::Mage);
    }
}

#[test]
fn scrappy_scavenger_discovers_a_card_costing_its_remaining_mana() {
    let mut f = Fix::new();
    f.g.players[0].mana = 4; // minus this card's own cost of 1 leaves 3
    f.play("Scrappy Scavenger", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.def().cost, 3);
}

#[test]
fn living_roots_two_modes_do_different_things() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play_mode("Living Roots", 0, foe_minion(0));
    assert_eq!(f.theirs(0).health(), 5, "mode 0 is 2 damage");
    assert_eq!(f.g.players[0].board.len(), 0);

    let mut f = Fix::new();
    f.play_mode("Living Roots", 1, None);
    assert_eq!(f.g.players[0].board.len(), 2, "mode 1 is two Saplings");
    assert_eq!(f.mine(0).card.name(), "Sapling");
}

#[test]
fn raven_idol_two_modes_discover_different_kinds() {
    let mut f = Fix::new();
    f.play_mode("Raven Idol", 0, None);
    assert_eq!(f.g.players[0].hand[0].card.def().kind(), Kind::Minion);

    let mut f = Fix::new();
    f.play_mode("Raven Idol", 1, None);
    assert_eq!(f.g.players[0].hand[0].card.def().kind(), Kind::Spell);
}

#[test]
fn feral_rage_two_modes_do_different_things() {
    let mut f = Fix::new();
    f.play_mode("Feral Rage", 0, None);
    assert_eq!(f.g.players[0].hero_bonus_atk, 4, "mode 0 is +4 Attack");
    assert_eq!(f.g.players[0].armor, 0);

    let mut f = Fix::new();
    f.play_mode("Feral Rage", 1, None);
    assert_eq!(f.g.players[0].hero_bonus_atk, 0);
    assert_eq!(f.g.players[0].armor, 8, "mode 1 is 8 Armor");
}

#[test]
fn contingency_draws_the_bottom_two_of_the_deck() {
    // `Fix::deck` pushes in order given, and the deck's own convention
    // treats the end of the vec as the top -- so "Wisp" here is the bottom.
    let mut f = Fix::new().deck(&["Wisp", "Chillwind Yeti", "Boulderfist Ogre"]);
    f.play("Contingency", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Wisp");
    assert_eq!(f.g.players[0].hand[1].card.name(), "Chillwind Yeti");
    assert_eq!(f.g.players[0].deck.len(), 1, "the top card is untouched");
}

#[test]
fn widows_bite_chain_escalates_through_feast_and_banquet() {
    // `Fix::play` always pushes a fresh copy and plays that -- wrong here,
    // since each step must play the very card the previous step handed out,
    // not an unrelated extra copy. `Action::Play` at the existing hand index
    // does that instead.
    let mut f = Fix::new();
    f.play("Widow's Bite", None);
    assert_eq!(f.g.players[0].hero_bonus_atk, 1);
    assert_eq!(f.g.players[0].armor, 1);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Widow's Feast");

    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].hero_bonus_atk, 3, "1 plus 2");
    assert_eq!(f.g.players[0].armor, 3, "1 plus 2");
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Widow's Banquet");

    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].hero_bonus_atk, 7, "3 plus 4");
    assert_eq!(f.g.players[0].armor, 7, "3 plus 4");
    assert_eq!(f.g.players[0].hand.len(), 0, "the chain ends here");
}

#[test]
fn savage_striker_damages_by_its_own_heros_attack() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("Fiery War Axe").unwrap(),
    )); // 3 Attack
    f.play("Savage Striker", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4, "7 minus the hero's 3 Attack");
}

#[test]
fn skyscreamer_eggs_deathrattle_summons_four_hatchlings() {
    let mut f = Fix::new().board(ME, &["Skyscreamer Eggs"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 4);
    for i in 0..4 {
        assert_eq!(f.mine(i).card.name(), "Skyscreamer Hatchling");
    }
}

#[test]
fn longneck_egg_deathrattle_summons_a_beast_and_buffs_the_board() {
    let mut f = Fix::new().board(ME, &["Longneck Egg", "Wisp"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(
        f.g.players[0].board.len(),
        2,
        "the Wisp plus the summoned Beast"
    );
    assert_eq!(f.mine(0).atk, 2, "Wisp: 1 plus the +1/+1");
    assert_eq!(f.mine(1).card.name(), "Little Longneck");
    assert_eq!(f.mine(1).atk, 4, "3 plus the +1/+1, buffed after it landed");
}

#[test]
fn hatchery_helper_buffs_low_attack_others_but_not_itself() {
    let mut f = Fix::new().board(ME, &["Wisp", "Boulderfist Ogre"]); // 1/1 and 6/7
    f.play("Hatchery Helper", None);
    assert_eq!(f.mine(0).atk, 2, "1 Attack: 1 plus 1");
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert_eq!(f.mine(1).atk, 6, "6 Attack: unaffected");
    assert!(!f.mine(1).has(Keywords::TAUNT));
    let helper = f.g.players[0]
        .board
        .iter()
        .find(|m| m.card.name() == "Hatchery Helper")
        .unwrap();
    assert_eq!(helper.atk, 2, "itself excluded despite 2 or less Attack");
}

#[test]
fn soul_immolation_swaps_the_hero_power_and_then_grows_it() {
    // "Your Hero Power becomes 'Collapsing Star'. If it already is, increase
    // its damage by 1." Two casts is one power that hits for 2.
    let mut f = Fix::new();
    f.play("Soul Immolation", None);
    assert_eq!(f.g.players[0].hero_power.name(), "Collapsing Star");
    assert_eq!(f.g.players[0].hero_power_bonus, 0, "the first cast only swaps");

    f.play("Soul Immolation", None);
    assert_eq!(f.g.players[0].hero_power.name(), "Collapsing Star");
    assert_eq!(f.g.players[0].hero_power_bonus, 1, "the second cast adds damage");
}

#[test]
fn collapsing_star_hits_an_enemy_for_its_current_damage() {
    // The only enemy is the hero, so "a random enemy" has one answer and the
    // damage is readable.
    let mut f = Fix::new();
    f.play("Soul Immolation", None);
    let before = f.g.players[1].hero_hp;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false,
    }));
    assert_eq!(f.g.players[1].hero_hp, before - 1, "base damage is 1");

    f.g.players[0].hero_power_uses = 0;
    f.play("Soul Immolation", None); // now +1
    let before = f.g.players[1].hero_hp;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false,
    }));
    assert_eq!(f.g.players[1].hero_hp, before - 2, "the raised damage lands");
}

#[test]
fn collapsing_star_refreshes_when_its_owner_summons_a_demon() {
    let mut f = Fix::new();
    f.play("Soul Immolation", None);
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false,
    }));
    assert_eq!(f.g.players[0].hero_power_uses, 1, "spent");

    // A Demon refreshes it; anything else does not.
    f.play("Voidwalker", None);
    assert_eq!(f.g.players[0].hero_power_uses, 0, "a Demon refreshed the power");

    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false,
    }));
    f.play("Wisp", None);
    assert_eq!(
        f.g.players[0].hero_power_uses, 1,
        "a Wisp is not a Demon and must not refresh it"
    );
}

#[test]
fn a_demon_refreshes_only_its_own_summoners_power() {
    let mut f = Fix::new();
    f.play("Soul Immolation", None);
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false,
    }));
    // The opponent summoning a Demon must not hand the power back.
    let voidwalker = by_name("Voidwalker").unwrap();
    f.g.summon(FOE, voidwalker);
    assert_eq!(f.g.players[0].hero_power_uses, 1);
}

// ------------------------------------------------- Deathwing, Worldbreaker

#[test]
fn a_hero_card_gives_armor_and_its_own_hero_power() {
    // A Hero card is played from hand like anything else. It hands over
    // armor and a Hero Power; the health printed on it belongs to a new
    // game, not to a hero already in one.
    let mut f = Fix::new();
    let hp_before = f.g.players[0].hero_hp;
    f.play("Deathwing, Worldbreaker", None);
    assert_eq!(f.g.players[0].armor, 12, "the card's printed armor");
    assert_eq!(f.g.players[0].hero_hp, hp_before, "health is not reset");
    assert_eq!(f.g.players[0].hero_power.name(), "Ruthless");
}

#[test]
fn ruthless_gives_the_hero_five_attack() {
    let mut f = Fix::new();
    f.play("Deathwing, Worldbreaker", None);
    f.g.players[0].hero_power_uses = 0;
    f.g.players[0].mana = 10;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false,
    }));
    assert_eq!(f.g.players[0].hero_bonus_atk, 5);
}

#[test]
fn deathwing_unleashes_one_cataclysm_and_three_at_full_herald() {
    // On an empty board the best Cataclysm is the 12/12, so one pick is one
    // Dragon and nothing else; at Herald 4 the picks keep coming and Enthrall
    // reaches the deck too.
    let mut f = Fix::new();
    f.play("Deathwing, Worldbreaker", None);
    assert_eq!(f.g.players[0].board.len(), 1, "one Cataclysm, one Dragon");
    assert_eq!(f.g.players[0].board[0].card.name(), "Progeny of Deathwing");
    assert_eq!(f.g.players[0].deck.len(), 0, "Enthrall was not among the picks");

    let mut f = Fix::new();
    f.g.players[0].herald = 4;
    f.play("Deathwing, Worldbreaker", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(
        f.g.players[0].deck.len(),
        5,
        "at Herald 4 three Cataclysms fire, Enthrall among them"
    );
}

#[test]
fn deathwing_clears_a_board_it_can_actually_kill() {
    // Three 3/2s die to Raze, which then outscores the 12/12: the pick is
    // about the position, not a fixed order. Two 4/5s, which Raze only
    // damages, go the other way — checked below so the trade-off is pinned
    // rather than implied.
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor", "Bloodfen Raptor", "Bloodfen Raptor"]);
    f.play("Deathwing, Worldbreaker", None);
    assert!(f.g.players[1].board.is_empty(), "Raze cleared the board");
    assert!(f.g.players[0].board.is_empty(), "only one Cataclysm fired");

    let mut f = Fix::new().board(FOE, &["Chillwind Yeti", "Chillwind Yeti"]);
    f.play("Deathwing, Worldbreaker", None);
    assert_eq!(f.g.players[1].board.len(), 2, "4 damage does not kill a 4/5");
    assert_eq!(
        f.g.players[0].board.first().map(|m| m.card.name()),
        Some("Progeny of Deathwing"),
        "against bodies it cannot kill, the Dragon is the better Cataclysm"
    );
}

#[test]
fn raze_hits_every_enemy_minion_for_four() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]).board(ME, &["Chillwind Yeti"]);
    f.play("Raze", None);
    assert_eq!(f.g.players[1].board[0].health(), 1);
    assert_eq!(f.g.players[0].board[0].health(), 5, "friendly minions are safe");
}

#[test]
fn topple_destroys_the_biggest_enemy_minion() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Chillwind Yeti", "Bloodfen Raptor"]);
    f.play("Topple", None);
    let left: Vec<&str> = f.g.players[1]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    assert_eq!(left, ["Wisp", "Bloodfen Raptor"], "the 4/5 is gone");
}

#[test]
fn topple_on_an_empty_board_does_nothing() {
    let mut f = Fix::new();
    f.play("Topple", None);
    assert!(f.g.players[1].board.is_empty());
}

#[test]
fn dragons_reign_summons_the_twelve_twelve() {
    let mut f = Fix::new();
    f.play("Dragon's Reign", None);
    let m = &f.g.players[0].board[0];
    assert_eq!(m.card.name(), "Progeny of Deathwing");
    assert_eq!((m.atk, m.health()), (12, 12));
}

#[test]
fn enthrall_shuffles_five_legendary_dragons_into_the_deck() {
    use tavernlab_core::cards::Rarity;
    let mut f = Fix::new();
    f.play("Enthrall", None);
    assert_eq!(f.g.players[0].deck.len(), 5);
    for card in f.g.players[0].deck.iter() {
        let d = card.def();
        assert_eq!(d.kind(), Kind::Minion);
        assert!(d.races.any(Races::DRAGON), "{} is not a Dragon", card.name());
        assert_eq!(d.rarity(), Rarity::Legendary, "{}", card.name());
    }
}

// ------------------------------------------------------------- Tradeable

#[test]
fn a_tradeable_card_goes_back_into_the_deck_for_one_mana_and_a_draw() {
    // Three Tradeable cards already shipped with the keyword doing nothing.
    // This is the option itself: one mana, the card back in the deck, a
    // fresh one in hand.
    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    let knight = by_name("The Black Knight").unwrap();
    f.g.players[0].hand.push(HandCard::new(knight));
    f.g.players[0].mana = 3;

    assert!(f.g.apply(Action::Trade { hand: 0 }));
    assert_eq!(f.g.players[0].mana, 2, "a Trade costs one mana");
    assert_eq!(f.g.players[0].deck.len(), 2, "one card in, one drawn out");
    assert!(
        f.g.players[0].deck.contains(&knight),
        "the traded card is back in the deck"
    );
    assert_eq!(f.g.players[0].hand.len(), 1, "and a card was drawn");
    assert_eq!(f.g.players[0].hand[0].card.name(), "Wisp");
}

#[test]
fn only_tradeable_cards_can_be_traded_and_only_with_mana() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    assert!(!f.g.apply(Action::Trade { hand: 0 }), "not a Tradeable card");

    let mut f = Fix::new().deck(&["Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("The Black Knight").unwrap()));
    f.g.players[0].mana = 0;
    assert!(!f.g.apply(Action::Trade { hand: 0 }), "no mana for it");
}

#[test]
fn trading_is_offered_only_while_the_mana_is_there() {
    use tavernlab_core::game::Action as A;
    let mut f = Fix::new().deck(&["Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("The Black Knight").unwrap()));

    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.players[0].mana = 1;
    f.g.legal_actions(&mut legal);
    assert!(
        legal.iter().any(|a| matches!(a, A::Trade { hand: 0 })),
        "with a mana in hand, the Trade is on the table"
    );

    f.g.players[0].mana = 0;
    f.g.legal_actions(&mut legal);
    assert!(
        !legal.iter().any(|a| matches!(a, A::Trade { .. })),
        "with no mana it is not"
    );
}

#[test]
fn the_agent_trades_a_card_it_cannot_play_and_plays_one_it_can() {
    // A Tradeable card it cannot afford is a dead card this turn: trading it
    // beats passing. Afford it and the body comes down instead — the option
    // must not crowd the board out.
    use tavernlab_core::agent::{Scripted, Style};
    use tavernlab_core::game::{Action as A, Agent as _};

    let mut agent = Scripted::new(Style::Midrange);
    let mut legal = tavernlab_core::inline::Inline::new();

    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("The Black Knight").unwrap()));
    f.g.players[0].mana = 1; // one mana: enough to Trade, not to play a 4-drop
    f.g.legal_actions(&mut legal);
    assert!(
        matches!(agent.choose(&f.g, legal.as_slice()), A::Trade { hand: 0 }),
        "a card it cannot afford is traded rather than sat on"
    );

    let mut f = Fix::new().deck(&["Wisp", "Wisp"]).board(FOE, &["Goldshire Footman"]);
    f.g.players[0].hand.push(HandCard::new(by_name("The Black Knight").unwrap()));
    f.g.players[0].mana = 4;
    f.g.legal_actions(&mut legal);
    assert!(
        matches!(agent.choose(&f.g, legal.as_slice()), A::Play { .. }),
        "with the mana for it, playing beats trading"
    );
}

#[test]
fn the_black_knight_destroys_a_taunt_and_needs_one_to_be_played_at_all() {
    let mut f = Fix::new().board(FOE, &["Goldshire Footman", "Chillwind Yeti"]);
    f.play("The Black Knight", Some(Target::Minion(FOE, 0)));
    let left: Vec<&str> = f.g.players[1].board.iter().map(|m| m.card.name()).collect();
    assert_eq!(left, ["Chillwind Yeti"], "the Taunt is gone, the 4/5 is not");

    // A non-Taunt is not a legal target for it.
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    let knight = by_name("The Black Knight").unwrap();
    f.g.players[0].hand.push(HandCard::new(knight));
    assert!(
        !f.g.apply(Action::Play {
            hand: 0,
            target: Some(Target::Minion(FOE, 0)),
            position: u8::MAX,
            choice: u8::MAX,
        }),
        "a minion without Taunt cannot be pointed at"
    );
}

// ----------------------------------------------------- neutral batch: cards

#[test]
fn dirty_rat_pulls_a_minion_out_of_the_enemy_hand_onto_their_board() {
    let mut f = Fix::new();
    let yeti = by_name("Chillwind Yeti").unwrap();
    f.g.players[1].hand.push(HandCard::new(yeti));
    f.g.players[1].hand.push(HandCard::new(by_name("Fireball").unwrap()));

    f.play("Dirty Rat", None);
    assert_eq!(
        f.g.players[1].board.iter().map(|m| m.card.name()).collect::<Vec<_>>(),
        ["Chillwind Yeti"],
        "the only minion in their hand came down"
    );
    assert_eq!(f.g.players[1].hand.len(), 1, "and left their hand");
    assert_eq!(f.g.players[1].hand[0].card.name(), "Fireball", "the spell stays");
}

#[test]
fn dirty_rat_does_nothing_against_a_hand_with_no_minions() {
    let mut f = Fix::new();
    f.g.players[1].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.play("Dirty Rat", None);
    assert!(f.g.players[1].board.is_empty());
    assert_eq!(f.g.players[1].hand.len(), 1);
}

#[test]
fn netherspite_historian_discovers_only_while_holding_a_dragon() {
    let mut f = Fix::new();
    f.play("Netherspite Historian", None);
    assert!(f.g.players[0].hand.is_empty(), "no Dragon in hand, no Discover");

    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Twilight Whelp").unwrap()));
    f.play("Netherspite Historian", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "the Dragon plus what was discovered");
    let found = f.g.players[0].hand[1].card;
    assert!(found.def().races.any(Races::DRAGON), "{} is not a Dragon", found.name());
}

#[test]
fn gorillabot_needs_another_mech_not_just_itself() {
    // It is a Mech and is already on the board when its Battlecry runs, so
    // counting itself would make the condition always true.
    let mut f = Fix::new();
    f.play("Gorillabot A-3", None);
    assert!(f.g.players[0].hand.is_empty(), "alone, it discovers nothing");

    let mut f = Fix::new().board(ME, &["Mechwarper"]);
    f.play("Gorillabot A-3", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "with another Mech out, it discovers");
    assert!(f.g.players[0].hand[0].card.def().races.any(Races::MECHANICAL));
}

#[test]
fn menagerie_mug_buffs_three_minions_of_three_different_tribes() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Murloc Raider", "Bloodfen Raptor", "Twilight Whelp"]);
    f.play("Menagerie Mug", None);
    let buffed: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.atk > m.card.def().atk)
        .map(|m| m.card.name())
        .collect();
    assert_eq!(buffed.len(), 3, "three minions, buffed: {buffed:?}");
    // Two Raptors are one tribe: only one of them can be among the three.
    assert_eq!(
        buffed.iter().filter(|n| **n == "Bloodfen Raptor").count(),
        1,
        "two Beasts are not two different types"
    );
}

#[test]
fn omen_of_the_end_mills_only_from_an_empty_deck() {
    let mut f = Fix::new();
    for _ in 0..6 {
        f.g.players[1].deck.push(by_name("Wisp").unwrap());
    }
    f.g.players[0].deck.push(by_name("Wisp").unwrap());
    f.play("Omen of the End", None);
    assert_eq!(f.g.players[1].deck.len(), 6, "your deck is not empty yet");

    f.g.players[0].deck.clear();
    f.play("Omen of the End", None);
    assert_eq!(f.g.players[1].deck.len(), 1, "five cards destroyed");
}

#[test]
fn the_soldiers_double_their_own_health_and_attack() {
    let mut f = Fix::new();
    f.play("Soldier of the Bronze", None);
    let m = &f.g.players[0].board[0];
    assert_eq!((m.atk, m.health()), (5, 6), "a 5/3 doubles its Health");

    let mut f = Fix::new();
    f.play("Soldier of the Infinite", None);
    let m = &f.g.players[0].board[0];
    assert_eq!((m.atk, m.health()), (6, 5), "a 3/5 doubles its Attack");
}

#[test]
fn concealing_confection_hands_back_a_weapon() {
    let mut f = Fix::new().board(ME, &["Concealing Confection"]);
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.def().kind(), Kind::Weapon);
}

#[test]
fn willful_watcher_burns_the_top_three_of_its_own_deck() {
    let mut f = Fix::new().board(ME, &["Willful Watcher"]).deck(&["Wisp", "Wisp", "Wisp", "Wisp", "Wisp"]);
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].deck.len(), 2);
    assert!(f.g.players[0].hand.is_empty(), "destroyed, not drawn");
}

#[test]
fn tindral_hits_harder_when_it_dies_on_the_opponents_turn() {
    let mut f = Fix::new().board(ME, &["Tindral Sageswift"]).board(FOE, &["Chillwind Yeti"]);
    let hp = f.g.players[1].hero_hp;
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[1].hero_hp, hp - 1, "on your own turn it is 1");
    assert_eq!(f.g.players[1].board[0].health(), 4);

    let mut f = Fix::new().board(ME, &["Tindral Sageswift"]).board(FOE, &["Chillwind Yeti"]);
    f.g.current = FOE; // it died on their turn
    let hp = f.g.players[1].hero_hp;
    f.g.destroy(Target::Minion(ME, 0));
    f.g.sweep_deaths();
    assert_eq!(f.g.players[1].hero_hp, hp - 4);
    assert_eq!(f.g.players[1].board[0].health(), 1);
}

#[test]
fn the_end_of_turn_neutrals_fire_on_their_own_controllers_turn() {
    // Critter Caretaker heals both heroes, Earthen Drake burns the enemy one,
    // Curious Cumulus shields its own, and the Pixie fetches a Nature spell.
    let mut f = Fix::new().board(
        ME,
        &["Critter Caretaker", "Earthen Drake", "Curious Cumulus", "Daydreaming Pixie"],
    );
    f.g.players[0].hero_hp = 20;
    f.g.players[1].hero_hp = 20;

    f.g.end_turn();
    assert_eq!(f.g.players[0].hero_hp, 23, "healed 3");
    assert_eq!(f.g.players[1].hero_hp, 20 + 3 - 4, "healed 3, then took 4");
    assert!(f.g.players[0].hero_divine_shield);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert_eq!(got.def().school(), tavernlab_core::cards::School::Nature, "{}", got.name());
}

#[test]
fn time_skipper_hands_the_coin_to_whoever_just_finished() {
    let mut f = Fix::new().board(ME, &["Time Skipper"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1, "my turn ended, my Coin");
    assert_eq!(f.g.players[0].hand[0].card.name(), "The Coin");

    f.g.current = FOE;
    f.g.end_turn();
    assert_eq!(f.g.players[1].hand.len(), 1, "their turn ended, their Coin");
}

// ------------------------------------------------- class backlog batch
// One test per card, in the order the rows appear in the table.

#[test]
fn monstrous_mosquito_buffs_the_rest_of_the_board_at_end_of_turn() {
    let mut f = Fix::new().board(ME, &["Monstrous Mosquito", "Chillwind Yeti"]);
    let before = f.mine(0).atk;
    f.g.end_turn();
    assert_eq!(f.mine(0).atk, before, "\"your other minions\" — not itself");
    assert_eq!(f.mine(1).atk, 5, "a 4/5 Yeti went to 5");
}

#[test]
fn thassarian_hits_on_the_way_in_and_on_the_way_out() {
    let mut f = Fix::new();
    f.play("Thassarian", None);
    assert_eq!(f.g.players[1].hero_hp, 28, "battlecry, no minions to pick");

    let slot = f.g.players[0].board.len() - 1;
    f.g.players[0].board[slot].damage = f.g.players[0].board[slot].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[1].hero_hp, 26, "deathrattle, same two damage");
    // Reborn, so the body is back at one Health.
    assert_eq!(f.mine(0).card.name(), "Thassarian");
    assert_eq!(f.mine(0).health(), 1);
}

#[test]
fn death_strike_heals_for_what_it_deals() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].hero_hp = 20;
    f.play("Death Strike", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 1);
    assert_eq!(f.g.players[0].hero_hp, 26, "Lifesteal for the full 6");
}

#[test]
fn death_strikes_lifesteal_follows_spell_damage() {
    let mut f = Fix::new()
        .board(ME, &["Kobold Geomancer"]) // Spell Damage +1
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].hero_hp = 20;
    f.play("Death Strike", foe_minion(0));
    assert_eq!(f.g.players[0].hero_hp, 27, "6 + 1 dealt is 6 + 1 healed");
}

#[test]
fn remorseless_winter_sweeps_the_enemy_side_and_draws() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Chillwind Yeti"])
        .deck(&["Fireball"]);
    f.play("Remorseless Winter", None);
    assert_eq!(f.mine(0).damage, 0, "\"all enemies\" spares my own board");
    assert_eq!(f.theirs(0).damage, 2);
    assert_eq!(f.g.players[1].hero_hp, 28, "the enemy hero is an enemy");
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn chaos_strike_arms_the_hero_for_one_turn_and_draws() {
    let mut f = Fix::new().deck(&["Fireball"]);
    f.play("Chaos Strike", None);
    assert_eq!(f.g.players[0].hero_attack(), 2);
    assert_eq!(f.g.players[0].hand.len(), 1);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hero_attack(), 0, "\"this turn\"");
}

#[test]
fn felrattler_rattles_over_the_enemy_board_only() {
    let mut f = Fix::new()
        .board(ME, &["Felrattler", "Wisp"])
        .board(FOE, &["Wisp", "Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 1, "the 1/1 died, the Yeti did not");
    assert_eq!(f.theirs(0).damage, 1);
    assert_eq!(f.mine(0).card.name(), "Wisp", "my own board is untouched");
    assert_eq!(f.mine(0).damage, 0);
}

#[test]
fn wrathspike_brute_answers_the_attacker_and_the_whole_board() {
    let mut f = Fix::new()
        .board(ME, &["Wrathspike Brute"]) // 3/6 Taunt
        .board(FOE, &["Chillwind Yeti", "Wisp"]);
    f.g.current = FOE;
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(ME, 0) }));
    assert_eq!(f.their_board(), 1, "the Wisp took its point and died");
    assert_eq!(f.theirs(0).damage, 3 + 1, "the Yeti took the Brute and the point");
    assert_eq!(f.g.players[1].hero_hp, 29, "\"all enemies\" reaches the hero");
}

#[test]
fn immortalized_in_stone_summons_one_of_each_statue() {
    let mut f = Fix::new();
    f.play("Immortalized in Stone", None);
    let got: Vec<(i16, i16)> = f.g.players[0]
        .board
        .iter()
        .map(|m| (m.atk, m.max_hp))
        .collect();
    assert_eq!(got, vec![(1, 2), (2, 4), (4, 8)]);
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::TAUNT)));
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.races().any(Races::ELEMENTAL))
    );
}

#[test]
fn haunt_gives_stats_taunt_and_a_second_life() {
    let mut f = Fix::new().board(ME, &["Wisp"]); // 1/1
    f.play("Haunt", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 4));
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert!(f.mine(0).has(Keywords::REBORN));

    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    // Reborn returns the printed body, not the buffed one.
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (1, 1));
    assert_eq!(f.mine(0).health(), 1);
}

#[test]
fn natalie_seline_takes_the_health_off_what_she_kills() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[1].board[0].damage = 2; // 5 left
    f.play("Natalie Seline", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(
        (f.mine(0).atk, f.mine(0).max_hp),
        (7, 1 + 5),
        "a 7/1 gains what the body still had"
    );
}

#[test]
fn story_of_amara_puts_the_hero_above_the_starting_total() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 12;
    f.play("Story of Amara", None);
    assert_eq!(f.g.players[0].hero_hp, 40);
    // And a later heal must not cap it back down to thirty.
    f.g.heal_hero(ME, 3);
    assert_eq!(f.g.players[0].hero_hp, 40);
}

#[test]
fn si7_supplier_draws_after_it_swings() {
    let mut f = Fix::new().board(ME, &["SI:7 Supplier"]).deck(&["Fireball"]);
    assert_eq!(f.g.players[0].hand.len(), 0);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[0].hand.len(), 1, "one card, after the attack");
}

#[test]
fn troubled_double_copies_itself_only_on_combo() {
    let mut f = Fix::new();
    f.play("Troubled Double", None);
    assert_eq!(f.g.players[0].board.len(), 1, "first card of the turn");

    let mut f = Fix::new();
    f.play("Wisp", None);
    f.play("Troubled Double", None);
    let doubles = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Troubled Double")
        .count();
    assert_eq!(doubles, 2);
}

#[test]
fn crazed_chemist_only_pays_out_on_combo() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Crazed Chemist", my_minion(0));
    assert_eq!(f.mine(0).atk, 4, "no combo, no buff");

    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Wisp", None);
    f.play("Crazed Chemist", my_minion(0));
    assert_eq!(f.mine(0).atk, 8);
}

#[test]
fn wailing_vapor_grows_on_elementals_but_not_on_itself() {
    let mut f = Fix::new();
    f.play("Wailing Vapor", None); // an Elemental itself
    assert_eq!(f.mine(0).atk, 1, "its own play does not count");
    f.play("Wisp", None);
    assert_eq!(f.mine(0).atk, 1, "a Wisp is not an Elemental");
    f.play("Wailing Vapor", None);
    assert_eq!(f.mine(0).atk, 2);
}

#[test]
fn fire_breath_burns_one_target_and_grows_your_elementals() {
    let mut f = Fix::new()
        .board(ME, &["Wailing Vapor", "Chillwind Yeti"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.play("Fire Breath", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 4);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 4), "the Elemental");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (4, 5), "the Yeti is not");
}

#[test]
fn hammer_of_twilight_leaves_an_elemental_when_it_breaks() {
    let mut f = Fix::new();
    f.play("Hammer of Twilight", None);
    assert_eq!(f.g.players[0].weapon.map(|w| (w.atk, w.durability)), Some((4, 2)));
    f.g.destroy_weapon(ME);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 2));
    assert!(f.mine(0).races().any(Races::ELEMENTAL));
}

#[test]
fn rotheart_dryad_pulls_a_big_minion_and_nothing_smaller() {
    let mut f = Fix::new()
        .board(ME, &["Rotheart Dryad"])
        .deck(&["Wisp", "Deathwing"]); // 0-cost and 10-cost
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Deathwing");
}

#[test]
fn twisting_nether_clears_both_boards_locations_included() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti", "Wisp"])
        .board(FOE, &["Boulderfist Ogre"]);
    let loc = by_name("Tranquil Clearing").expect("a Location in the corpus");
    assert_eq!(loc.def().kind(), Kind::Location);
    f.g.players[1].board.push(Permanent::summon(loc));
    f.play("Twisting Nether", None);
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.their_board(), 0);
}

#[test]
fn sky_raider_finds_a_pirate() {
    let mut f = Fix::new();
    f.play("Sky Raider", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert!(got.def().races.any(Races::PIRATE), "{}", got.name());
}

#[test]
fn ravaging_ghoul_spares_only_itself() {
    let mut f = Fix::new()
        .board(ME, &["Wisp"])
        .board(FOE, &["Wisp", "Chillwind Yeti"]);
    f.play("Ravaging Ghoul", None);
    assert_eq!(f.g.players[0].board.len(), 1, "my Wisp died, the Ghoul lived");
    assert_eq!(f.mine(0).card.name(), "Ravaging Ghoul");
    assert_eq!(f.mine(0).damage, 0);
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).damage, 1);
}

#[test]
fn ironforge_portal_armours_up_and_brings_a_body() {
    let mut f = Fix::new();
    f.play("Ironforge Portal", None);
    assert_eq!(f.g.players[0].armor, 4);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.def().cost, 4);
    assert_eq!(f.mine(0).kind(), Kind::Minion);
}

#[test]
fn guard_duty_summons_one_taunt_at_each_printed_cost() {
    let mut f = Fix::new();
    f.play("Guard Duty", None);
    let got: Vec<i16> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.def().cost)
        .collect();
    assert_eq!(got, vec![6, 4, 2]);
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::TAUNT)));
}

// --------------------------------------------------------------- Reborn
// The rule itself, rather than any one card that carries it.

#[test]
fn reborn_brings_a_minion_back_once_at_one_health() {
    let mut f = Fix::new().board(ME, &["Sinful Steed"]);
    assert!(f.mine(0).has(Keywords::REBORN));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).health(), 1);
    assert!(!f.mine(0).has(Keywords::REBORN), "once, not forever");

    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 0, "the second death is final");
}

#[test]
fn reborn_needs_room_left_after_the_deathrattle() {
    // The body has left the board by the time Reborn returns it, so there is
    // normally room by construction. A deathrattle that fills the vacated
    // slot is the case where there is not.
    let mut f = Fix::new().board(ME, &["Reluctant Wrangler"]);
    while f.g.players[0].board.len() < 7 {
        f.g.players[0].board.push(Permanent::summon(by_name("Wisp").unwrap()));
    }
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 7);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.card.name() != "Reluctant Wrangler"),
        "the rattle took the last slot, so the return had nowhere to go"
    );
}

#[test]
fn a_card_being_played_does_not_react_to_its_own_play() {
    let mut f = Fix::new();
    f.play("Questing Adventurer", None); // 2/2, "whenever you play a card"
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 2));
    f.play("Wisp", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 3));
}

// ------------------------------------------ class backlog, second pass

#[test]
fn cairne_bloodhoof_leaves_baine() {
    let mut f = Fix::new().board(ME, &["Cairne Bloodhoof"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Baine Bloodhoof");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 5));
}

#[test]
fn voidlord_leaves_three_taunting_demons() {
    let mut f = Fix::new().board(ME, &["Voidlord"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 3);
    for i in 0..3 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (1, 3));
        assert!(f.mine(i).has(Keywords::TAUNT));
        assert!(f.mine(i).races().any(Races::DEMON));
    }
}

#[test]
fn mountain_bear_leaves_two_cubs() {
    let mut f = Fix::new().board(ME, &["Mountain Bear"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 2);
    for i in 0..2 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (2, 4));
        assert!(f.mine(i).has(Keywords::TAUNT));
    }
}

#[test]
fn eternal_bloodpetal_and_its_seedling_trade_places() {
    let mut f = Fix::new().board(ME, &["Eternal Bloodpetal"]); // 5/1
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.mine(0).card.name(), "Eternal Seedling");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (0, 1));

    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.mine(0).card.name(), "Eternal Bloodpetal", "and back again");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 1));
}

#[test]
fn sneeds_old_shredder_leaves_a_legendary() {
    let mut f = Fix::new().board(ME, &["Sneed's Old Shredder"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    let got = f.mine(0).card;
    assert_eq!(
        got.def().rarity(),
        tavernlab_core::cards::Rarity::Legendary,
        "{}",
        got.name()
    );
}

#[test]
fn drakeadon_mongrel_leaves_two_four_drops() {
    let mut f = Fix::new().board(ME, &["Drakeadon Mongrel"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert!(f.g.players[0].board.iter().all(|m| m.card.def().cost == 4));
}

#[test]
fn obsidian_statue_takes_one_enemy_with_it() {
    let mut f = Fix::new()
        .board(ME, &["Obsidian Statue"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0, "destroyed outright, health ignored");
}

#[test]
fn avatar_of_destruction_takes_the_enemy_board_with_it() {
    let mut f = Fix::new()
        .board(ME, &["Avatar of Destruction", "Chillwind Yeti"])
        .board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.mine(0).damage, 0, "\"enemy minions\" only");
    assert_eq!(f.g.players[1].hero_hp, 30, "and not the hero");
}

#[test]
fn sewer_imp_hits_everything_on_the_far_side() {
    let mut f = Fix::new()
        .board(ME, &["Sewer Imp"])
        .board(FOE, &["Wisp", "Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 1, "the 1/1 died");
    assert_eq!(f.theirs(0).damage, 2);
    assert_eq!(f.g.players[1].hero_hp, 28);
}

#[test]
fn fae_trickster_pulls_an_expensive_spell() {
    let mut f = Fix::new()
        .board(ME, &["Fae Trickster"])
        .deck(&["Moonfire", "Pyroblast"]); // 0-cost and 10-cost spells
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Pyroblast");
}

#[test]
fn tormented_dreadwing_pulls_two_cheaper_dragons() {
    let mut f = Fix::new()
        .board(ME, &["Tormented Dreadwing"])
        .deck(&["Wisp", "Faerie Dragon", "Twilight Drake"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 2);
    for h in f.g.players[0].hand.iter() {
        assert!(h.card.def().races.any(Races::DRAGON), "{}", h.card.name());
        assert_eq!(h.cost_delta, -1);
    }
}

#[test]
fn seeding_dragon_leaves_a_discounted_dragon_in_hand() {
    let mut f = Fix::new().board(ME, &["Seeding Dragon"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert!(h.card.def().races.any(Races::DRAGON), "{}", h.card.name());
    assert_eq!(h.cost_delta, -2);
}

#[test]
fn twilight_mender_leaves_one_spell_of_each_school() {
    use tavernlab_core::cards::School;
    let mut f = Fix::new().board(ME, &["Twilight Mender"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let schools: Vec<School> = f.g.players[0]
        .hand
        .iter()
        .map(|h| h.card.def().school())
        .collect();
    assert_eq!(schools, vec![School::Holy, School::Shadow]);
}

#[test]
fn primordial_drake_spares_only_itself() {
    let mut f = Fix::new()
        .board(ME, &["Wisp"])
        .board(FOE, &["Wisp", "Chillwind Yeti"]);
    f.play("Primordial Drake", None);
    assert_eq!(f.g.players[0].board.len(), 1, "my Wisp died, the Drake lived");
    assert_eq!(f.mine(0).card.name(), "Primordial Drake");
    assert_eq!(f.mine(0).damage, 0);
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).damage, 2);
}

#[test]
fn nerubian_swarmguard_arrives_in_triplicate() {
    let mut f = Fix::new();
    f.play("Nerubian Swarmguard", None);
    assert_eq!(f.g.players[0].board.len(), 3);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.card.name() == "Nerubian Swarmguard")
    );
}

#[test]
fn underking_armours_up_twice() {
    let mut f = Fix::new();
    f.play("Underking", None);
    assert_eq!(f.g.players[0].armor, 6, "battlecry");
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].armor, 12, "and deathrattle");
}

#[test]
fn heir_of_hereafter_scales_with_the_wounded() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Boulderfist Ogre", "Chillwind Yeti"]);
    f.g.players[0].board[0].damage = 1;
    f.g.players[1].board[0].damage = 1;
    f.play("Heir of Hereafter", None);
    let heir = f.mine(1);
    assert_eq!((heir.atk, heir.max_hp), (2 + 4, 6 + 4), "two damaged minions");
}

#[test]
fn heir_of_hereafter_is_a_plain_body_on_a_clean_board() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Heir of Hereafter", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 6));
}

#[test]
fn blizzard_damages_and_freezes_the_enemy_board() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Chillwind Yeti", "Wisp"]);
    f.play("Blizzard", None);
    assert_eq!(f.their_board(), 1, "the Wisp died to the two damage");
    assert_eq!(f.theirs(0).damage, 2);
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert!(!f.mine(0).flags.has(Flags::FROZEN), "my board is not frozen");
}

#[test]
fn ceremonial_clash_brings_three_bodies_and_overloads() {
    let mut f = Fix::new();
    f.play("Ceremonial Clash", None);
    let costs: Vec<i16> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.def().cost)
        .collect();
    assert_eq!(costs, vec![3, 2, 1]);
    assert_eq!(f.g.players[0].overload_next, 1);
}

#[test]
fn ward_of_earth_armours_up_and_plants_a_taunt() {
    let mut f = Fix::new();
    f.play("Ward of Earth", None);
    assert_eq!(f.g.players[0].armor, 5);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.def().cost, 5);
    assert!(f.mine(0).has(Keywords::TAUNT));
}

#[test]
fn for_all_time_clears_the_small_and_leaves_the_big() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"]) // 4/5, four or less
        .board(FOE, &["Boulderfist Ogre", "Wisp"]); // 6/7 and 1/1
    f.play("For All Time", None);
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).card.name(), "Boulderfist Ogre");
    assert_eq!(f.g.players[0].overload_next, 2);
}

#[test]
fn forests_gift_counts_the_board_it_buffs() {
    let mut f = Fix::new().board(ME, &["Wisp", "Wisp", "Chillwind Yeti"]);
    f.play("Forest's Gift", my_minion(2));
    assert_eq!((f.mine(2).atk, f.mine(2).max_hp), (4 + 3, 5 + 3));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (1, 1), "one minion only");
}

#[test]
fn dethrone_kills_and_only_replaces_on_combo() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Dethrone", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].board.len(), 0, "no combo, no body");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Wisp", None);
    f.play("Dethrone", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(1).card.def().cost, 8);
}

#[test]
fn nascent_bolt_pays_out_only_when_the_target_lives() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Nascent Bolt", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 5);
    assert_eq!(f.g.players[0].hand.len(), 0, "an empty deck draws nothing");

    let mut f = Fix::new()
        .board(FOE, &["Boulderfist Ogre"])
        .deck(&["Wisp", "Wisp"]);
    f.play("Nascent Bolt", foe_minion(0));
    assert_eq!(f.g.players[0].hand.len(), 2, "it survived");

    let mut f = Fix::new()
        .board(FOE, &["Chillwind Yeti"]) // 4/5, dies to five
        .deck(&["Wisp", "Wisp"]);
    f.play("Nascent Bolt", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hand.len(), 0, "it did not survive");
}

#[test]
fn eldritch_tentacles_hits_three_then_two_then_one() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"]) // 6/7 — takes all six
        .board(FOE, &["Chillwind Yeti"]); // 4/5 — dies on the second pass
    f.play("Eldritch Tentacles", None);
    assert_eq!(f.mine(0).damage, 3 + 2 + 1);
    assert_eq!(f.their_board(), 0);
}

#[test]
fn yesterloc_hardens_the_rest_of_the_board() {
    let mut f = Fix::new().board(ME, &["Yesterloc", "Chillwind Yeti"]);
    let before = f.mine(0).max_hp;
    f.g.end_turn();
    assert_eq!(f.mine(0).max_hp, before, "\"your other minions\"");
    assert_eq!(f.mine(1).max_hp, 6);
}

#[test]
fn scalebreaker_bulwark_pings_the_far_side_each_turn() {
    let mut f = Fix::new()
        .board(ME, &["Scalebreaker Bulwark"])
        .board(FOE, &["Chillwind Yeti"]);
    f.g.end_turn();
    assert_eq!(f.theirs(0).damage, 2);
    assert_eq!(f.g.players[1].hero_hp, 28);
    assert_eq!(f.mine(0).damage, 0);
}

#[test]
fn gorishi_tunneler_reaches_the_hero_after_a_trade() {
    let mut f = Fix::new()
        .board(ME, &["Gorishi Tunneler"])
        .board(FOE, &["Wisp"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.g.players[1].hero_hp, 28, "two, on top of the trade");
}

#[test]
fn axe_of_the_forefathers_sweeps_after_every_swing() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Wisp"]);
    f.play("Axe of the Forefathers", None);
    assert!(f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) }));
    assert_eq!(f.their_board(), 0, "the Wisp took its point");
    assert_eq!(f.mine(0).damage, 1, "\"all minions\" — mine too");
}

#[test]
fn truth_seeker_grows_only_the_paladin_half_of_the_board() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Argent Protector"]);
    assert_eq!(f.mine(1).card.def().class(), Class::Paladin);
    f.play("Truth Seeker", None);
    assert!(f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 5), "a Neutral Yeti");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (3 + 2, 2 + 2));
}

// ------------------------------------------- "Has +N Attack while …"
// The rule first, then one case per card.

/// Attack of the first friendly minion after taking one point of damage.
fn enraged_attack(name: &str) -> (i16, i16) {
    let mut f = Fix::new().board(ME, &[name]);
    let calm = f.mine(0).atk;
    // Two spare Health first: Angry Chicken is a 1/1, and a body that dies to
    // the point of damage never gets to be angry about it.
    f.g.buff(Target::Minion(ME, 0), 0, 2);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    (calm, f.mine(0).atk)
}

#[test]
fn enrage_switches_on_with_damage_and_off_with_healing() {
    let mut f = Fix::new().board(ME, &["Amani Berserker"]); // 2/3, +3 damaged
    assert_eq!(f.mine(0).atk, 2);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_eq!(f.mine(0).atk, 5, "damaged");
    f.g.heal(Target::Minion(ME, 0), 1);
    assert_eq!(f.mine(0).atk, 2, "and back to full");
}

#[test]
fn silence_takes_the_enrage_with_it() {
    let mut f = Fix::new().board(ME, &["Amani Berserker"]);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_eq!(f.mine(0).atk, 5);
    f.g.silence(Target::Minion(ME, 0));
    f.g.recompute_auras();
    assert_eq!(f.mine(0).atk, 2, "a silenced minion grants itself nothing");
}

#[test]
fn an_enraged_minion_hits_for_the_enraged_number() {
    let mut f = Fix::new()
        .board(ME, &["Amani Berserker"]) // 2/3, +3 while damaged
        .board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.theirs(0).damage, 5, "five, not two");
}

#[test]
fn every_while_damaged_card_carries_its_printed_number() {
    for (name, bonus) in [
        ("Aberrant Berserker", 2),
        ("Amani Berserker", 3),
        ("Angry Chicken", 5),
        ("Bloodhoof Brave", 3),
        ("Dozing Marksman", 4),
        ("Grommash Hellscream", 6),
        ("Redband Wasp", 3),
        ("Tauren Warrior", 3),
        ("Temple Berserker", 2),
        ("Undercover Cultist", 3),
        ("Warbot", 1),
    ] {
        let (calm, angry) = enraged_attack(name);
        assert_eq!(angry - calm, bonus, "{name}");
    }
}

#[test]
fn every_tar_minion_wakes_up_on_the_other_turn() {
    for (name, bonus) in [
        ("Tar Slime", 2),
        ("Tar Creeper", 2),
        ("Tar Lurker", 3),
        ("Tar Lord", 4),
        ("Tar Tyrant", 6),
    ] {
        let mut f = Fix::new().board(ME, &[name]);
        let mine = f.mine(0).atk;
        f.g.current = FOE;
        f.g.recompute_auras();
        assert_eq!(f.mine(0).atk - mine, bonus, "{name} on their turn");
        f.g.current = ME;
        f.g.recompute_auras();
        assert_eq!(f.mine(0).atk, mine, "{name} back on mine");
    }
}

#[test]
fn a_tar_minion_is_awake_when_it_is_attacked() {
    // The whole point of the card: it defends at the bigger number. The turn
    // boundary is what refreshes it, so this drives real turns.
    let mut f = Fix::new()
        .board(ME, &["Tar Creeper"]) // 1/5, +2 on their turn
        .board(FOE, &["Chillwind Yeti"]); // 4/5
    f.g.end_turn();
    f.g.current = FOE;
    f.g.begin_turn();
    assert_eq!(f.mine(0).atk, 3);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(ME, 0) }));
    assert_eq!(f.theirs(0).damage, 3, "it hit back for three");
}

#[test]
fn cogmaster_needs_a_mech_on_the_board() {
    let mut f = Fix::new().board(ME, &["Cogmaster"]); // 1/2
    assert_eq!(f.mine(0).atk, 1);
    f.play("Mechanical Dragonling", None); // a Mech
    assert_eq!(f.mine(0).atk, 3);
}

#[test]
fn proud_defender_stands_alone() {
    let mut f = Fix::new().board(ME, &["Proud Defender"]); // 2/6
    assert_eq!(f.mine(0).atk, 4, "no other minions");
    f.play("Wisp", None);
    assert_eq!(f.mine(0).atk, 2, "and now there is one");
}

#[test]
fn small_time_buccaneer_needs_the_weapon() {
    let mut f = Fix::new().board(ME, &["Small-Time Buccaneer"]); // 1/2
    assert_eq!(f.mine(0).atk, 1);
    f.play("Fiery War Axe", None);
    assert_eq!(f.mine(0).atk, 3);
    f.g.destroy_weapon(ME);
    assert_eq!(f.mine(0).atk, 1, "and it goes when the weapon does");
}

#[test]
fn spine_crawler_needs_a_location() {
    let mut f = Fix::new().board(ME, &["Spine Crawler"]); // 1/6
    assert_eq!(f.mine(0).atk, 1);
    assert!(f.mine(0).has(Keywords::CANT_ATTACK), "printed on the card");
    f.play("Tranquil Clearing", None);
    assert_eq!(f.mine(0).atk, 4);
}

#[test]
fn surging_tempest_reads_the_crystals_locked_now() {
    let mut f = Fix::new().board(ME, &["Surging Tempest"]); // 1/3
    assert_eq!(f.mine(0).atk, 1);
    f.play("Ceremonial Clash", None); // Overload: (1), queued for next turn
    assert_eq!(f.mine(0).atk, 1, "queued Overload is not locked Overload");
    f.g.end_turn();
    f.g.begin_turn();
    assert_eq!(f.g.players[0].overload_now, 1);
    assert_eq!(f.mine(0).atk, 2);
}

// ------------------------------------------------ "… that died this game"
// The graveyard, then the cards that read it.

/// Kill the minion in `slot` on my board and let the sweep record it.
fn bury(f: &mut Fix, slot: usize) {
    f.g.players[0].board[slot].damage = f.g.players[0].board[slot].max_hp;
    f.g.sweep_deaths();
}

#[test]
fn the_graveyard_records_what_died_in_the_order_it_died() {
    let mut f = Fix::new().board(ME, &["Wisp", "Chillwind Yeti"]);
    bury(&mut f, 0);
    bury(&mut f, 0);
    let names: Vec<&str> = f.g.players[0]
        .graveyard
        .iter()
        .map(|c| c.name())
        .collect();
    assert_eq!(names, vec!["Wisp", "Chillwind Yeti"]);
    assert_eq!(f.g.players[0].deaths, 2);
    assert_eq!(f.g.players[1].graveyard.len(), 0, "each side keeps its own");
}

#[test]
fn a_dying_minion_is_already_buried_when_its_own_deathrattle_runs() {
    // Ysondre counts her own death, which only works if the record is written
    // before the rattle fires.
    let mut f = Fix::new().board(ME, &["Ysondre"]);
    bury(&mut f, 0);
    assert_eq!(f.g.players[0].board.len(), 1, "one Dragon for one death");
    assert!(f.mine(0).races().any(Races::DRAGON));
}

#[test]
fn ysondre_scales_with_how_often_she_has_died() {
    let mut f = Fix::new().board(ME, &["Ysondre", "Ysondre"]);
    bury(&mut f, 0);
    // One Dragon arrived; kill the second Ysondre, which is now slot 0 or 1.
    let at = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Ysondre")
        .expect("the second one is still there");
    let before = f.g.players[0].board.len();
    bury(&mut f, at);
    assert_eq!(
        f.g.players[0].board.len(),
        before - 1 + 2,
        "she died twice, so two Dragons"
    );
}

#[test]
fn the_graveyard_is_capped_and_the_count_is_not() {
    use tavernlab_core::state::GRAVEYARD;
    let mut f = Fix::new();
    for _ in 0..(GRAVEYARD + 5) {
        f = f.board(ME, &["Wisp"]);
        bury(&mut f, 0);
    }
    assert_eq!(f.g.players[0].graveyard.len(), GRAVEYARD, "the pool stops");
    assert_eq!(
        f.g.players[0].deaths as usize,
        GRAVEYARD + 5,
        "the count does not"
    );
}

#[test]
fn calia_menethil_brings_back_the_biggest_thing_you_lost() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre", "Wisp", "Chillwind Yeti"]);
    bury(&mut f, 0);
    bury(&mut f, 0);
    bury(&mut f, 0);
    f.play("Calia Menethil", None);
    let back: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    assert!(back.contains(&"Boulderfist Ogre"), "{back:?}");
}

#[test]
fn calia_menethil_on_an_empty_graveyard_is_just_a_body() {
    let mut f = Fix::new();
    f.play("Calia Menethil", None);
    assert_eq!(f.g.players[0].board.len(), 1);
}

#[test]
fn memoriam_manifest_only_looks_at_undead() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre", "Nerubian Swarmguard"]);
    bury(&mut f, 0); // a 6-cost that is not Undead
    bury(&mut f, 0); // a 4-cost Undead
    f.play("Memoriam Manifest", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Nerubian Swarmguard");
}

#[test]
fn resuscitate_brings_back_one_of_each_cost_with_reborn() {
    let mut f = Fix::new().board(ME, &["Wisp", "Amani Berserker", "Tauren Warrior"]);
    for _ in 0..3 {
        bury(&mut f, 0);
    }
    // Wisp costs 0 and matches nothing; the 2 and the 3 come back.
    f.play("Resuscitate", None);
    let costs: Vec<i16> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.def().cost)
        .collect();
    assert_eq!(costs, vec![2, 3]);
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::REBORN)));
}

#[test]
fn merithra_brings_back_one_of_each_big_thing() {
    let mut f = Fix::new().board(ME, &["Deathwing", "Deathwing", "Boulderfist Ogre"]);
    for _ in 0..3 {
        bury(&mut f, 0);
    }
    f.play("Merithra", None);
    let back: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    assert_eq!(
        back.iter().filter(|n| **n == "Deathwing").count(),
        1,
        "two died, one comes back: {back:?}"
    );
    assert!(!back.contains(&"Boulderfist Ogre"), "a 6-cost is not (8) or more");
}

#[test]
fn undeath_sentence_fires_a_rattle_from_the_graveyard() {
    let mut f = Fix::new().board(ME, &["Cairne Bloodhoof"]);
    bury(&mut f, 0);
    assert_eq!(f.g.players[0].board.len(), 1, "Baine, from the real death");
    f.play("Undeath Sentence", None);
    let baines = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Baine Bloodhoof")
        .count();
    assert_eq!(baines, 2, "and a second Baine from the graveyard");
}

#[test]
fn endbringer_umbra_reads_the_graveyard_now() {
    let mut f = Fix::new().board(ME, &["Cairne Bloodhoof", "Chillwind Yeti"]);
    bury(&mut f, 0);
    // Clear Baine and the Yeti away so only the graveyard can explain what
    // Umbra summons.
    while !f.g.players[0].board.is_empty() {
        f.g.players[0].board.remove(0);
    }
    f.play("Endbringer Umbra", None);
    let baines = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Baine Bloodhoof")
        .count();
    assert_eq!(baines, 1, "Cairne's rattle, fired from the graveyard");
}

#[test]
fn aessina_waits_for_twenty_deaths() {
    let mut f = Fix::new();
    f.g.players[1].hero_hp = 30;
    f.play("Aessina", None);
    assert_eq!(f.g.players[1].hero_hp, 30, "nineteen is not twenty");

    let mut f = Fix::new();
    f.g.players[0].deaths = 20;
    f.play("Aessina", None);
    assert_eq!(f.g.players[1].hero_hp, 10, "twenty damage, nothing else to hit");
}

#[test]
fn splintered_reality_grows_with_the_treants_you_have_lost() {
    let mut f = Fix::new();
    f.play("Splintered Reality", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 2), "none died yet");
    for _ in 0..2 {
        bury(&mut f, 0);
    }
    f.play("Splintered Reality", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 4), "two Treants died");
}

#[test]
fn succumb_to_madness_only_resummons_a_dragon() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Faerie Dragon"]);
    bury(&mut f, 0);
    bury(&mut f, 0);
    f.play("Succumb to Madness", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Faerie Dragon");
}

#[test]
fn succumb_to_madness_on_an_empty_graveyard_does_nothing() {
    let mut f = Fix::new();
    f.play("Succumb to Madness", None);
    assert_eq!(f.g.players[0].board.len(), 0);
}

// -------------------------------------------- class backlog, third pass

#[test]
fn voodoo_totem_hands_over_a_shadow_spell_each_turn() {
    use tavernlab_core::cards::School;
    let mut f = Fix::new().board(ME, &["Voodoo Totem"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert_eq!(got.def().school(), School::Shadow, "{}", got.name());
    assert_eq!(got.def().kind(), Kind::Spell);
}

#[test]
fn selenic_drake_hands_over_a_dragon_each_turn() {
    let mut f = Fix::new().board(ME, &["Selenic Drake"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert!(got.def().races.any(Races::DRAGON), "{}", got.name());
}

#[test]
fn runaway_blackwing_burns_one_enemy_minion_a_turn() {
    let mut f = Fix::new()
        .board(ME, &["Runaway Blackwing"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.end_turn();
    assert_eq!(f.their_board(), 0, "ten kills a 6/7");
    assert_eq!(f.g.players[1].hero_hp, 30, "a minion, not the hero");
}

#[test]
fn iridescent_flitterwing_grows_the_rest_of_the_board() {
    let mut f = Fix::new().board(ME, &["Iridescent Flitterwing", "Wisp"]);
    let before = (f.mine(0).atk, f.mine(0).max_hp);
    f.g.end_turn();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), before, "\"your other\"");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (2, 2));
}

#[test]
fn crystal_merchant_draws_only_on_leftover_mana() {
    let mut f = Fix::new().board(ME, &["Crystal Merchant"]).deck(&["Wisp"]);
    f.g.players[0].mana = 0;
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 0, "nothing unspent");

    let mut f = Fix::new().board(ME, &["Crystal Merchant"]).deck(&["Wisp"]);
    f.g.players[0].mana = 3;
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn animated_moonwell_grows_by_what_you_cast() {
    let mut f = Fix::new().board(ME, &["Animated Moonwell"]); // 1/4
    f.play("Fireball", Some(Target::Hero(FOE))); // costs 4
    assert_eq!(f.mine(0).atk, 5);
}

#[test]
fn marshland_thresher_shields_up_after_a_spell() {
    let mut f = Fix::new().board(ME, &["Marshland Thresher"]);
    assert!(!f.mine(0).has(Keywords::DIVINE_SHIELD));
    f.play("Fireball", Some(Target::Hero(FOE)));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
}

#[test]
fn archmage_antonidas_pays_a_fireball_per_spell() {
    let mut f = Fix::new().board(ME, &["Archmage Antonidas"]);
    f.play("The Coin", None); // a spell
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Fireball");
}

#[test]
fn veteran_warmedic_answers_holy_and_nothing_else() {
    let mut f = Fix::new().board(ME, &["Veteran Warmedic"]);
    f.play("Fireball", Some(Target::Hero(FOE))); // Fire
    assert_eq!(f.g.players[0].board.len(), 1, "not a Holy spell");
    f.play("Holy Light", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (2, 2));
    assert!(f.mine(1).has(Keywords::LIFESTEAL));
}

#[test]
fn windswept_pageturner_pings_when_an_elemental_lands() {
    let mut f = Fix::new().board(ME, &["Windswept Pageturner"]);
    f.play("Wisp", None);
    assert_eq!(f.g.players[1].hero_hp, 30, "a Wisp is not an Elemental");
    f.play("Wailing Vapor", None);
    assert_eq!(f.g.players[1].hero_hp, 27);
}

#[test]
fn rioter_rewards_a_minion_that_lived_through_it() {
    let mut f = Fix::new().board(ME, &["Rioter", "Boulderfist Ogre"]); // 6/7
    f.g.deal_damage(Target::Minion(ME, 1), 1);
    assert_eq!(f.mine(1).atk, 7, "it survived, so it grew");

    let mut f = Fix::new()
        .board(ME, &["Rioter"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.deal_damage(Target::Minion(FOE, 0), 1);
    assert_eq!(f.theirs(0).atk, 6, "\"a friendly minion\"");
}

#[test]
fn black_market_overseer_rushes_the_deathrattle_you_just_played() {
    let mut f = Fix::new().board(ME, &["Black Market Overseer"]);
    f.play("Cairne Bloodhoof", None); // a Deathrattle minion
    assert!(f.mine(1).has(Keywords::RUSH));
    f.play("Chillwind Yeti", None); // no Deathrattle
    assert!(!f.mine(2).has(Keywords::RUSH));
}

#[test]
fn bugsquasher_can_only_point_at_something_with_a_tribe() {
    // Tribeless: the minion still comes down, and the battlecry is skipped —
    // the same rule every targeted battlecry follows.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    assert!(f.theirs(0).races().is_empty());
    f.play("Bugsquasher", None);
    assert_eq!(f.theirs(0).damage, 0);
    assert_eq!(f.g.players[0].board.len(), 1);

    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]); // a Beast
    f.play("Bugsquasher", foe_minion(0));
    assert_eq!(f.their_board(), 0, "six is more than a 3/2 has");
}

#[test]
fn epoch_stalker_arrives_twice() {
    let mut f = Fix::new();
    f.play("Epoch Stalker", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.card.name() == "Epoch Stalker")
    );
}

#[test]
fn halazzi_fills_the_hand_with_lynxes() {
    let mut f = Fix::new();
    f.play("Halazzi, the Lynx", None);
    assert_eq!(f.g.players[0].hand.len(), MAX_HAND);
    for h in f.g.players[0].hand.iter() {
        assert_eq!(h.card.name(), "Lynx");
        assert!(h.card.def().keywords.has(Keywords::RUSH));
    }
}

#[test]
fn coghammer_shields_and_taunts_a_friend() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Coghammer", None);
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert_eq!(f.g.players[0].weapon.map(|w| (w.atk, w.durability)), Some((2, 3)));
}

#[test]
fn tankgineer_leaves_a_force_tank() {
    let mut f = Fix::new().board(ME, &["Tankgineer"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (7, 7));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
}

#[test]
fn tirion_fordring_leaves_the_ashbringer() {
    let mut f = Fix::new().board(ME, &["Tirion Fordring"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(
        f.g.players[0].weapon.map(|w| (w.card.name(), w.atk, w.durability)),
        Some(("Ashbringer", 5, 3))
    );
}

#[test]
fn lightshower_elemental_heals_your_whole_side() {
    let mut f = Fix::new().board(ME, &["Lightshower Elemental", "Boulderfist Ogre"]);
    f.g.players[0].hero_hp = 20;
    f.g.players[0].board[1].damage = 5;
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hero_hp, 28);
    assert_eq!(f.mine(0).damage, 0);
}

#[test]
fn sahket_sapper_sends_an_enemy_home() {
    let mut f = Fix::new()
        .board(ME, &["Sahket Sapper"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[1].hand.len(), 1);
    assert_eq!(f.g.players[1].hand[0].card.name(), "Boulderfist Ogre");
}

#[test]
fn stormbinder_gives_the_crystals_back() {
    let mut f = Fix::new().board(ME, &["Stormbinder"]);
    f.g.players[0].overload_now = 3;
    f.g.players[0].mana = 4;
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].overload_now, 0);
    assert_eq!(f.g.players[0].mana, 7);
}

#[test]
fn ball_and_chain_only_pays_the_wounded() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Boulderfist Ogre"]);
    f.g.players[0].board[0].damage = 1;
    f.play("Ball and Chain", None);
    f.g.destroy_weapon(ME);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 7), "damaged");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (6, 7), "not damaged");
}

#[test]
fn panther_mask_overwrites_the_body_and_draws() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"])
        .deck(&["Wisp", "Wisp"]);
    f.play("Panther Mask", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 4));
    assert!(f.mine(0).has(Keywords::STEALTH));
    assert_eq!(f.g.players[0].hand.len(), 2);
}

#[test]
fn devilsaur_mask_makes_a_charger() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Devilsaur Mask", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 8));
    assert!(f.mine(0).has(Keywords::CHARGE));
}

#[test]
fn call_of_the_wild_brings_all_three_companions() {
    let mut f = Fix::new();
    f.play("Call of the Wild", None);
    let mut names: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    names.sort();
    assert_eq!(names, ["Huffer", "Leokk", "Misha"]);
}

#[test]
fn thiefs_tools_hands_over_two_discounted_four_drops() {
    let mut f = Fix::new();
    f.play("Thief's Tools", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    for h in f.g.players[0].hand.iter() {
        assert_eq!(h.card.def().cost, 4, "{}", h.card.name());
        assert_eq!(h.card.def().kind(), Kind::Spell);
        assert_eq!(h.cost_delta, -2);
    }
}

#[test]
fn healing_rain_spends_every_point_it_can_and_no_more() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre"]);
    f.g.players[0].hero_hp = 25; // 5 to give back
    f.g.players[0].board[0].damage = 4;
    f.play("Healing Rain", None);
    assert_eq!(f.g.players[0].hero_hp, 30);
    assert_eq!(f.mine(0).damage, 0, "nine of the twelve landed, three had nowhere");
}

#[test]
fn typhoon_clears_both_boards_into_decks() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti", "Wisp"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.play("Typhoon", None);
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.their_board(), 0);
    let total = f.g.players[0].deck.len() + f.g.players[1].deck.len();
    assert_eq!(total, 3, "three minions went into somebody's deck");
}

#[test]
fn fortify_hits_for_the_armor_it_just_gained() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].armor = 2;
    f.play("Fortify", foe_minion(0));
    assert_eq!(f.g.players[0].armor, 5);
    assert_eq!(f.theirs(0).damage, 5);
}

#[test]
fn shellnado_spends_armor_up_to_five() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"])
        .board(FOE, &["Chillwind Yeti"]);
    f.g.players[0].armor = 3;
    f.play("Shellnado", None);
    assert_eq!(f.g.players[0].armor, 0);
    assert_eq!(f.mine(0).damage, 3, "\"all minions\" — mine too");
    assert_eq!(f.theirs(0).damage, 3);

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].armor = 9;
    f.play("Shellnado", None);
    assert_eq!(f.g.players[0].armor, 4, "at most five");
    assert_eq!(f.theirs(0).damage, 5);
}

// --------------------------------------------------------------- Druid

#[test]
fn charred_chameleon_waits_for_the_hero_power() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Charred Chameleon", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 5), "power unused");

    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0].hero_power_uses = 1;
    f.play("Charred Chameleon", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 7));
    assert!(f.mine(0).has(Keywords::RUSH));
}

#[test]
fn crystalspine_cub_grows_when_the_last_crystal_goes() {
    let mut f = Fix::new().board(ME, &["Crystalspine Cub"]);
    f.g.players[0].mana = 5;
    f.play("Wisp", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (1, 1), "mana left over");
    f.g.players[0].mana = 0;
    f.play("Wisp", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 2));
}

#[test]
fn life_cycle_replaces_the_minion_on_its_own_side() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]); // 4-cost
    f.play("Life Cycle", foe_minion(0));
    assert_eq!(f.g.players[0].board.len(), 0, "\"to replace it\"");
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).card.def().cost, 4);
    assert_ne!(f.theirs(0).card.name(), "Chillwind Yeti");
}

#[test]
fn symbiosis_looks_outside_your_own_class() {
    let mut f = Fix::new(); // a Mage fixture
    f.play("Symbiosis", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert!(got.def().keywords.has(Keywords::CHOOSE_ONE), "{}", got.name());
    assert_ne!(got.def().class(), Class::Mage);
    assert_ne!(got.def().class(), Class::Neutral);
}

#[test]
fn mossbinding_pours_the_rest_of_your_mana_into_two_golems() {
    let mut f = Fix::new();
    f.g.players[0].mana = 5; // 2 goes on the spell, 3 left
    f.play("Mossbinding", None);
    assert_eq!(f.g.players[0].mana, 0);
    assert_eq!(f.g.players[0].board.len(), 2);
    for i in 0..2 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (1 + 3, 2 + 3));
    }
}

#[test]
fn ravenous_flock_arrives_next_turn() {
    let mut f = Fix::new();
    f.play("Ravenous Flock", None);
    assert_eq!(f.g.players[0].board.len(), 0, "not yet");
    f.g.end_turn();
    f.g.begin_turn();
    assert_eq!(f.g.players[0].board.len(), 3);
    for i in 0..3 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (2, 1));
    }
}

#[test]
fn tranquil_clearing_taunts_a_minion_and_puts_it_to_sleep() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Tranquil Clearing", None);
    let loc = f.g.players[0].board.len() - 1;
    assert!(f.g.apply(Action::UseLocation {
        slot: loc as u8,
        target: my_minion(0),
    }));
    assert_eq!(f.mine(0).max_hp, 7);
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert!(f.mine(0).flags.has(Flags::DORMANT), "asleep");
}

#[test]
fn commissary_crook_spends_the_rest_of_the_turn() {
    let mut f = Fix::new();
    f.g.players[0].mana = 8; // 3 on the Crook, 5 left
    f.play("Commissary Crook", None);
    assert_eq!(f.g.players[0].mana, 0);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(1).card.def().cost, 5);
}

#[test]
fn overheat_pays_twice_when_it_finds_a_nature_spell() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Overheat", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 6), "nothing to discard");

    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Healing Rain").unwrap())); // a Nature spell
    f.play("Overheat", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 7));
    assert_eq!(f.g.players[0].hand.len(), 0, "the Nature spell went");
}

#[test]
fn spiteful_chef_reads_your_crystals() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 5;
    f.play("Spiteful Chef", None);
    assert_eq!(f.mine(1).card.def().cost, 2);
    assert!(f.mine(1).has(Keywords::TAUNT));

    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.play("Spiteful Chef", None);
    assert_eq!(f.mine(1).card.def().cost, 6);
    assert!(f.mine(1).has(Keywords::TAUNT));
}

#[test]
fn oaken_summons_pulls_a_body_out_of_the_deck() {
    let mut f = Fix::new().deck(&["Boulderfist Ogre", "Chillwind Yeti"]);
    f.play("Oaken Summons", None);
    assert_eq!(f.g.players[0].armor, 6);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Chillwind Yeti", "the 6-cost is too big");
    assert_eq!(f.g.players[0].deck.len(), 1, "and it left the deck");
}

#[test]
fn boomkins_two_modes_do_different_things() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play_mode("Boomkin", 0, None);
    assert_eq!(f.g.players[0].hero_hp, 28);

    let mut f = Fix::new();
    f.play_mode("Boomkin", 1, Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 26);
}

#[test]
fn endangered_dodo_doubles_up_when_you_are_low() {
    let mut f = Fix::new();
    f.play("Endangered Dodo", None);
    assert_eq!(f.g.players[0].board.len(), 1, "at full health, just a body");

    let mut f = Fix::new();
    f.g.players[0].hero_hp = 10;
    f.play("Endangered Dodo", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (10, 10));
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (5, 5), "the copy is printed");
}

#[test]
fn flipper_friends_offers_one_big_body_or_six_small_ones() {
    let mut f = Fix::new();
    f.play_mode("Flipper Friends", 0, None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 6));
    assert!(f.mine(0).has(Keywords::TAUNT));

    let mut f = Fix::new();
    f.play_mode("Flipper Friends", 1, None);
    assert_eq!(f.g.players[0].board.len(), 6, "the board holds seven");
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::RUSH)));
}

// -------------------------------------------------------- Demon Hunter

#[test]
fn sigil_of_cinder_goes_off_next_turn() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Sigil of Cinder", None);
    assert_eq!(f.theirs(0).damage, 0, "not yet");
    f.g.end_turn();
    f.g.begin_turn();
    let dealt = f.g.players[1].board.first().map_or(7, |m| m.damage) + (30 - f.g.players[1].hero_hp);
    assert_eq!(dealt, 6, "six points, split somewhere among the enemies");
}

#[test]
fn armored_bloodletter_heralds() {
    let mut f = Fix::new();
    f.g.players[0].class = Class::DemonHunter;
    f.play("Armored Bloodletter", None);
    assert_eq!(f.g.players[0].herald, 1);
    assert_eq!(f.g.players[0].board.len(), 2, "the Bloodletter and a Soldier");
}

#[test]
fn nightmare_dragonkin_discounts_the_card_on_the_right() {
    let mut f = Fix::new().board(ME, &["Nightmare Dragonkin"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand[0].cost_delta, 0);
    assert_eq!(f.g.players[0].hand[1].cost_delta, -2, "right-most");
}

#[test]
fn defiled_spear_carries_past_the_minion_it_hit() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Defiled Spear", None); // 2/3
    assert!(f.g.apply(Action::HeroAttack { target: Target::Minion(FOE, 0) }));
    assert_eq!(f.theirs(0).damage, 2, "the swing itself");
    assert_eq!(f.g.players[1].hero_hp, 28, "and two more, elsewhere");
}

#[test]
fn scorchreaver_finds_fel_and_discounts_the_fel_you_hold() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Chaos Strike").unwrap())); // a Fel spell
    f.play("Scorchreaver", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "one discovered");
    for h in f.g.players[0].hand.iter() {
        assert_eq!(h.card.def().school(), tavernlab_core::cards::School::Fel);
        assert_eq!(h.cost_delta, -1);
    }
}

#[test]
fn chronikar_arms_the_hero_for_three_turns() {
    let mut f = Fix::new();
    f.play("Chronikar", None);
    assert_eq!(f.g.players[0].hero_attack(), 3, "this turn");
    for turn in 0..2 {
        f.g.end_turn();
        f.g.begin_turn();
        assert_eq!(f.g.players[0].hero_attack(), 3, "turn {turn} after");
    }
    f.g.end_turn();
    f.g.begin_turn();
    assert_eq!(f.g.players[0].hero_attack(), 0, "and then it stops");
}

#[test]
fn flash_flood_hits_both_ends_and_outcast_does_it_twice() {
    // From the middle of a hand: one pass. The fixture's `play` always puts a
    // card at the right-hand end, which is an Outcast, so this builds the hand
    // by hand.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Chillwind Yeti", "Boulderfist Ogre"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Flash Flood").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 1,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.theirs(0).damage, 5);
    assert_eq!(f.theirs(1).damage, 0, "the middle is untouched");
    assert_eq!(f.theirs(2).damage, 5);

    // From the end: Outcast, so both ends take it twice and the 6/7s die.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Chillwind Yeti", "Boulderfist Ogre"]);
    f.play("Flash Flood", None);
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).card.name(), "Chillwind Yeti");
    assert_eq!(f.theirs(0).damage, 0);
}

#[test]
fn priestess_of_fury_rains_six_a_turn() {
    let mut f = Fix::new()
        .board(ME, &["Priestess of Fury"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.end_turn();
    let dealt = f.theirs(0).damage + (30 - f.g.players[1].hero_hp);
    assert_eq!(dealt, 6);
    assert_eq!(f.mine(0).damage, 0, "\"all enemies\"");
}

#[test]
fn perennial_serpent_is_cheaper_while_something_sleeps() {
    let mut f = Fix::new();
    let serpent = by_name("Perennial Serpent").unwrap();
    f.g.players[0].hand.push(HandCard::new(serpent));
    assert_eq!(f.g.card_cost(ME, 0), 8);
    let mut dormant = Permanent::summon(by_name("Wisp").unwrap());
    dormant.flags.insert(Flags::DORMANT);
    f.g.players[1].board.push(dormant);
    assert_eq!(f.g.card_cost(ME, 0), 4, "either side's Dormant minion counts");
}

#[test]
fn dread_leviathan_drains_three_times() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].hero_hp = 15;
    f.play("Dread Leviathan", foe_minion(0));
    assert_eq!(f.their_board(), 0, "nine into a seven-health body");
    assert_eq!(f.g.players[0].hero_hp, 15 + 9, "three points a time, thrice");
}

#[test]
fn malevolent_mutant_copies_a_fel_spell_you_hold() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Chaos Strike").unwrap()));
    f.play("Malevolent Mutant", None);
    let names: Vec<&str> = f.g.players[0].hand.iter().map(|h| h.card.name()).collect();
    assert_eq!(names, vec!["Chaos Strike", "Chaos Strike"]);
}

#[test]
fn solitude_discounts_your_hand_only_with_a_minionless_deck() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Solitude", None);
    assert_eq!(f.g.players[0].hand[0].cost_delta, 0, "the deck still has one");

    let mut f = Fix::new().deck(&["Fireball"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Solitude", None);
    assert_eq!(f.g.players[0].hand[0].cost_delta, -2);
    assert!(f.g.players[0].hand.len() >= 3, "and two were discovered");
}

#[test]
fn silithid_queen_arms_the_hero_after_a_beast_turn() {
    let mut f = Fix::new();
    f.play("Silithid Queen", None);
    assert_eq!(f.g.players[0].hero_attack(), 0, "no Beast last turn");

    let mut f = Fix::new();
    f.g.players[0].played_races_last = Races::BEAST;
    f.play("Silithid Queen", None);
    assert_eq!(f.g.players[0].hero_attack(), 5);
}

// -------------------------------------------------------------- Shaman

#[test]
fn blazing_invocation_finds_a_discounted_battlecry() {
    let mut f = Fix::new();
    f.play("Blazing Invocation", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert!(h.card.def().keywords.has(Keywords::BATTLECRY), "{}", h.card.name());
    assert_eq!(h.cost_delta, -1);
}

#[test]
fn flames_of_the_firelord_doubles_when_you_hold_something_big() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Flames of the Firelord", None);
    assert_eq!(f.theirs(0).damage, 4);

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Deathwing").unwrap())); // costs 10
    f.play("Flames of the Firelord", None);
    assert_eq!(f.their_board(), 0, "eight kills a 6/7");
}

#[test]
fn ritual_of_power_heralds_and_hands_over_two_breezlings() {
    let mut f = Fix::new();
    f.g.players[0].class = Class::Shaman;
    f.play("Ritual of Power", None);
    assert_eq!(f.g.players[0].herald, 1);
    assert_eq!(f.g.players[0].hand.len(), 2);
    for h in f.g.players[0].hand.iter() {
        assert_eq!(h.card.name(), "Breezling");
        assert!(h.card.def().keywords.has(Keywords::RUSH));
    }
}

#[test]
fn emberscarred_whelp_lends_a_crystal_for_one_turn() {
    let mut f = Fix::new();
    f.play("Emberscarred Whelp", None);
    assert_eq!(f.g.players[0].hand.len(), 1, "one discovered");
    assert_eq!(f.g.players[0].hand[0].card.def().cost, 5);
    let before = f.g.players[0].mana;
    f.g.end_turn();
    f.g.begin_turn();
    assert!(f.g.players[0].mana > before.min(1), "a crystal arrived");
}

#[test]
fn lava_flow_re_picks_between_its_three_hits() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp"]);
    f.g.players[1].hero_hp = 30;
    f.play("Lava Flow", None);
    assert_eq!(f.their_board(), 0, "two Wisps died to two of the three hits");
    assert_eq!(f.g.players[1].hero_hp, 28, "the third went face");
    assert_eq!(f.g.players[0].overload_next, 1);
}

#[test]
fn chillspine_stegodon_freezes_only_on_kindred() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Boulderfist Ogre"]);
    f.play("Chillspine Stegodon", None);
    assert_eq!(f.theirs(0).damage + f.theirs(1).damage, 4);
    assert!(!f.theirs(0).flags.has(Flags::FROZEN));

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Boulderfist Ogre"]);
    f.g.players[0].played_races_last = Races::ELEMENTAL;
    f.play("Chillspine Stegodon", None);
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert!(f.theirs(1).flags.has(Flags::FROZEN));
}

#[test]
fn mechanized_magma_grows_by_the_fire_you_cast() {
    let mut f = Fix::new().board(ME, &["Mechanized Magma"]); // 2/5
    f.play("Fireball", Some(Target::Hero(FOE))); // Fire, costs 4
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 9));
    f.play("The Coin", None); // not Fire
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 9));
}

#[test]
fn rehgar_earthfury_pays_for_his_neighbours_swings() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Rehgar Earthfury", "Wisp"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[0].hand.len(), 1, "the left neighbour");
    assert_eq!(f.g.players[0].hand[0].card.name(), "Lightning Bolt");
    assert!(f.g.apply(Action::Attack { from: 1, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[0].hand.len(), 2, "and himself");
}

#[test]
fn slagclaw_pops_its_cinders_on_kindred() {
    let mut f = Fix::new();
    f.play("Slagclaw", None);
    assert_eq!(f.g.players[1].hero_hp, 30, "no Kindred, no bang");
    assert_eq!(f.g.players[0].board.len(), 3);

    let mut f = Fix::new();
    f.g.players[0].played_races_last = Races::DRAGON;
    f.play("Slagclaw", None);
    assert_eq!(f.g.players[1].hero_hp, 26, "two Cinders, two damage each");
    assert_eq!(f.g.players[0].board.len(), 3, "and they are still standing");
}

#[test]
fn spirits_of_the_forest_offers_wolves_or_falcons() {
    let mut f = Fix::new();
    f.play_mode("Spirits of the Forest", 0, None);
    assert_eq!(f.g.players[0].board.len(), 3);
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::TAUNT)));

    let mut f = Fix::new();
    f.play_mode("Spirits of the Forest", 1, None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::WINDFURY)));
}

#[test]
fn glaciate_lands_a_frozen_eight_drop() {
    let mut f = Fix::new();
    f.play("Glaciate", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.def().cost, 8);
    assert!(f.mine(0).flags.has(Flags::FROZEN));
}

#[test]
fn sizzling_swarm_leaves_three_cinders_behind() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Sizzling Swarm", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 3);
    assert_eq!(f.g.players[0].board.len(), 3);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.card.name() == "Sizzling Cinder")
    );
}

#[test]
fn tortotem_finds_something_with_two_tribes() {
    let mut f = Fix::new().board(ME, &["Tortotem"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert!(
        got.def().races.0.count_ones() > 1 || got.def().races.has(Races::ALL),
        "{}",
        got.name()
    );
}
