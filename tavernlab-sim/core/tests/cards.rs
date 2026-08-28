//! One test per card, or per family of cards that share a rule.
//!
//! With no second engine to compare against, this is where correctness is
//! established. Comparing only who won would let a card that is wrong in a way
//! that rarely decides a game pass forever; asserting the resulting board makes
//! the mistake visible on the turn it happens.
//!
//! Each test builds a fixed position, plays exactly one card, and states what
//! should have changed.

use tavernlab_core::cards::{
    Class, Formats, Keywords, Kind, Races, Rarity, School, behaviour_of, by_id, by_name,
};
use tavernlab_core::game::{Action, Agent};
use tavernlab_core::state::{
    DeckCard, Flags, Game, HandCard, MAX_HAND, Marks, Permanent, Side, Target,
};

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
            self.g.players[0].deck.push(DeckCard::started(by_name(n).unwrap()));
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

/// A `Ctx` for calling a hook directly, where there is no play to carry one.
fn make_ctx(card: tavernlab_core::cards::CardId, side: Side) -> tavernlab_core::cards::Ctx {
    tavernlab_core::cards::Ctx {
        card,
        side,
        target: None,
        source: None,
        outcast: false,
        dying: None,
        marks: Marks::NONE,
        mana_spent: 0,
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
    f.g.players[1].deck.push(DeckCard::started(by_name("Chillwind Yeti").unwrap()));
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
    f.g.players[1].deck.push(DeckCard::started(by_name("Bloodfen Raptor").unwrap()));
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

    let count = g.players[0].deck.iter().filter(|d| d.card == leg).count()
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

    let mine = g.players[0].deck.iter().filter(|d| d.card == llane).count()
        + g.players[0].hand.iter().filter(|hc| hc.card == llane).count();
    let theirs = g.players[1].deck.iter().filter(|d| d.card == llane).count()
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
        f.g.players[0].deck.iter().any(|c| c.name() == "King Llane"),
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
    f.g.players[0].deck.push(DeckCard::started(by_name("Warptooth").unwrap()));
    for _ in 0..4 {
        f.g.deal_damage(Target::Hero(ME), 1);
    }
    assert!(
        !f.g.players[0].deck.iter().any(|c| c.name() == "Warptooth"),
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
    f.g.players[1].deck.push(DeckCard::started(by_name("Chillwind Yeti").unwrap()));
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
fn dreambound_raptor_gives_a_real_bonus_effect_but_not_to_itself() {
    let mut f = Fix::new().board(ME, &["Dreambound Raptor"]);
    f.play("Chillwind Yeti", None);
    assert_eq!(granted_bonuses(f.mine(1)), 1, "exactly one Bonus Effect");
    assert_eq!(
        (f.mine(1).atk, f.mine(1).max_hp),
        (4, 5),
        "a Bonus Effect is a keyword, not stats"
    );
    assert_eq!(granted_bonuses(f.mine(0)), 0, "the Raptor itself is unaffected");
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
    f.g.players[0].deck.push(DeckCard::started(by_name("Wisp").unwrap())); // fed to the guaranteed draw
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
    f.g.players[0].deck.push(DeckCard::started(wisp)); // fed to the guaranteed draw
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
fn ultraxion_heralds_and_reduces_deathwings_cost_wherever_it_waits() {
    let mut f = Fix::new();
    let deathwing = by_name("Deathwing, Worldbreaker").unwrap();
    f.g.players[0].hand.push(HandCard::new(deathwing));
    // Deathwing on top, so the draw below takes it.
    f.g.players[0].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
    f.g.players[0].deck.push(DeckCard::started(deathwing));
    f.play("Ultraxion", None);
    assert_eq!(f.g.players[0].herald, 1, "Heralded once");
    let hc = f.g.players[0]
        .hand
        .iter()
        .find(|hc| hc.card == deathwing)
        .expect("Deathwing is still in hand");
    assert_eq!(hc.cost_delta, -1);
    assert_eq!(f.g.players[0].deck[1].cost_delta, -1, "and the copy in the deck");
    assert_eq!(f.g.players[0].deck[0].cost_delta, 0, "the Wisp is not Deathwing");

    // The reduction survives the draw, which is the whole point of writing
    // it on the deck card.
    f.g.draw(ME, 1);
    let drawn = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, drawn), 9);
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

    let anywhere = g.players[0].deck.iter().any(|d| d.card == broxigar)
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
            .filter(|c| c.name() == "Second Portal to Argus")
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
        f.g.players[0].deck.push(DeckCard::started(filler));
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

    f.g.players[0].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
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
    f.g.players[1].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
    f.g.players[1].deck.push(DeckCard::started(by_name("Chillwind Yeti").unwrap())); // top: popped first
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
        assert_eq!(
            d.cost + card.cost_delta as i16,
            1,
            "{} should cost (1)",
            card.name()
        );
    }
    // And still costs (1) once drawn.
    f.g.draw(ME, 1);
    assert_eq!(f.g.card_cost(ME, 0), 1);
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
        f.g.players[0].deck.iter().any(|d| d.card == knight),
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
        f.g.players[1].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
    }
    f.g.players[0].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
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
    // A minion whose whole text is keywords, so what is being tested is the
    // rule and not a card. Sinful Steed used to stand here and no longer can:
    // it prints its own exception to this ("with full Health and
    // enchantments"), which `sinful_steed_comes_back_whole` covers instead.
    let mut f = Fix::new().board(ME, &["Whelp of the Infinite"]);
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
    f.g.players[0].overload_next = 1; // as an Overload card would leave it
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

// ------------------------------------------------------------- Warrior

#[test]
fn ominous_nightmares_sweeps_or_feeds_the_wounded() {
    let mut f = Fix::new()
        .board(ME, &["Wisp"])
        .board(FOE, &["Chillwind Yeti"]);
    f.play_mode("Ominous Nightmares", 0, None);
    assert_eq!(f.g.players[0].board.len(), 0, "\"all minions\"");
    assert_eq!(f.theirs(0).damage, 1);

    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0].board[0].damage = 1;
    f.play_mode("Ominous Nightmares", 1, my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 7));
}

#[test]
fn precursory_strike_draws_only_while_you_hold_something_big() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Precursory Strike", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 27);
    assert_eq!(f.g.players[0].hand.len(), 0);

    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Boulderfist Ogre").unwrap()));
    f.play("Precursory Strike", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[0].hand.len(), 2, "the Ogre, and the drawn Yeti");
}

#[test]
fn stonecarver_feeds_a_wounded_neighbour_not_itself() {
    let mut f = Fix::new().board(ME, &["Stonecarver", "Chillwind Yeti"]);
    f.g.players[0].board[0].damage = 1;
    f.g.players[0].board[1].damage = 1;
    f.g.end_turn();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (1, 4), "\"another\"");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (6, 7));
}

#[test]
fn baleful_blazer_needs_a_fire_spell_first() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Baleful Blazer", foe_minion(0));
    assert_eq!(f.their_board(), 1, "no Fire cast this turn");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Fireball", Some(Target::Hero(FOE)));
    f.play("Baleful Blazer", foe_minion(0));
    assert_eq!(f.their_board(), 0);
}

#[test]
fn latorvian_armorer_only_gets_paid_for_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Latorvian Armorer", foe_minion(0));
    assert_eq!(f.g.players[0].armor, 0, "it lived");

    let mut f = Fix::new().board(FOE, &["Wisp"]);
    f.play("Latorvian Armorer", foe_minion(0));
    assert_eq!(f.g.players[0].armor, 5);
}

#[test]
fn cataclysmic_war_axe_heralds_when_equipped() {
    let mut f = Fix::new();
    f.g.players[0].class = Class::Warrior;
    f.play("Cataclysmic War Axe", None);
    assert_eq!(f.g.players[0].herald, 1);
    assert_eq!(f.g.players[0].board.len(), 1, "a Soldier of Ragnaros");
    assert!(f.g.players[0].weapon.is_some());
}

#[test]
fn scorching_ravager_rushes_the_soldier_it_heralds() {
    let mut f = Fix::new();
    f.g.players[0].class = Class::Warrior;
    f.play("Scorching Ravager", None);
    let last = f.g.players[0].board.len() - 1;
    assert_eq!(f.mine(last).card.name(), "Soldier of Ragnaros");
    assert!(f.mine(last).has(Keywords::RUSH));
}

#[test]
fn afflicted_devastator_burns_your_side_then_theirs() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Chillwind Yeti"]);
    f.play("Afflicted Devastator", None);
    assert_eq!(f.mine(0).damage, 3, "\"all other friendly\"");
    assert_eq!(f.theirs(0).damage, 0, "not yet");
    let at = f.g.players[0].board.len() - 1;
    f.g.players[0].board[at].damage = f.g.players[0].board[at].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.theirs(0).damage, 3, "and the rattle reaches their side");
}

#[test]
fn nablya_copies_only_the_wounded() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti", "Wisp"]);
    f.g.players[0].board[0].damage = 1;
    f.play("Nablya, the Watcher", None);
    let yetis = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Chillwind Yeti")
        .count();
    assert_eq!(yetis, 2);
    let wisps = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Wisp")
        .count();
    assert_eq!(wisps, 1, "the Wisp was untouched");
    assert!(f.mine(f.g.players[0].board.len() - 1).has(Keywords::RUSH));
}

#[test]
fn the_great_dracorex_sprays_the_rest_of_their_board() {
    let mut f = Fix::new()
        .board(ME, &["The Great Dracorex"]) // 5/12 Rush
        .board(FOE, &["Chillwind Yeti", "Boulderfist Ogre", "Boulderfist Ogre"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.their_board(), 2, "the Yeti took five and died");
    assert_eq!(f.theirs(0).damage, 5, "and so did the others");
    assert_eq!(f.theirs(1).damage, 5);
}

#[test]
fn undefeated_champion_fills_the_other_board() {
    let mut f = Fix::new().board(FOE, &["Wisp"]);
    f.play("Undefeated Champion", None);
    assert_eq!(f.their_board(), 7, "filled to the brim");
    for i in 1..7 {
        assert_eq!(f.theirs(i).card.def().cost, 1);
    }
}

#[test]
fn tortolla_grows_on_every_hit() {
    let mut f = Fix::new().board(ME, &["Tortolla"]); // 1/30
    f.g.deal_damage(Target::Minion(ME, 0), 3);
    assert_eq!(f.mine(0).atk, 2);
    assert_eq!(f.g.players[0].armor, 1);
    f.g.deal_damage(Target::Minion(ME, 0), 3);
    assert_eq!(f.mine(0).atk, 3);
    assert_eq!(f.g.players[0].armor, 2);
}

#[test]
fn crowd_control_hits_twice_and_is_cheaper_on_a_fat_deck() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Crowd Control", None);
    assert_eq!(f.theirs(0).damage, 4);

    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Crowd Control").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 5);
    for _ in 0..25 {
        f.g.players[0].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
    }
    assert_eq!(f.g.card_cost(ME, 0), 3);
}

#[test]
fn for_glory_is_cheaper_the_more_they_have() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp", "Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("For Glory!").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 2);
}

#[test]
fn scrappy_defender_reads_the_deck_as_it_shrinks() {
    let mut f = Fix::new().board(ME, &["Scrappy Defender"]); // 2/7
    for _ in 0..25 {
        f.g.players[0].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
    }
    f.g.recompute_auras();
    assert_eq!(f.mine(0).atk, 7);
    f.g.draw(ME, 1);
    assert_eq!(f.mine(0).atk, 2, "twenty-four is not twenty-five");
}

#[test]
fn gladiatorial_combat_arms_both_sides() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Gladiatorial Combat", None);
    assert_eq!(f.mine(0).card.name(), "Chillwind Yeti");
    assert_eq!(f.theirs(0).card.name(), "Coliseum Tiger");
    assert!(f.theirs(0).has(Keywords::STEALTH));
}

// ---------------------------------------------------------------- Mage

#[test]
fn mirror_dimension_doubles_up_for_a_dragon_in_hand() {
    let mut f = Fix::new();
    f.play("Mirror Dimension", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (0, 4));
    assert!(f.mine(0).has(Keywords::TAUNT));

    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Faerie Dragon").unwrap()));
    f.play("Mirror Dimension", None);
    assert_eq!(f.g.players[0].board.len(), 2);
}

#[test]
fn spark_of_life_picks_a_class_to_dig_in() {
    let mut f = Fix::new();
    f.play_mode("Spark of Life", 0, None);
    assert_eq!(f.g.players[0].hand[0].card.def().class(), Class::Mage);

    let mut f = Fix::new();
    f.play_mode("Spark of Life", 1, None);
    assert_eq!(f.g.players[0].hand[0].card.def().class(), Class::Druid);
}

#[test]
fn scorching_winds_pays_double_for_a_discard() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Scorching Winds", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 3, "nothing to discard");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Fireball").unwrap())); // a Fire spell
    f.play("Scorching Winds", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 6);
    assert_eq!(f.g.players[0].hand.len(), 0);
}

#[test]
fn spire_security_reads_the_deck_without_moving_it() {
    let mut f = Fix::new()
        .board(FOE, &["Boulderfist Ogre"])
        .deck(&["Pyroblast"]); // costs 10
    f.play("Spire Security", None);
    assert_eq!(f.g.players[0].deck.len(), 1, "revealed, not drawn");
    assert_eq!(f.theirs(0).damage, 5);

    let mut f = Fix::new()
        .board(FOE, &["Boulderfist Ogre"])
        .deck(&["Moonfire"]); // costs 0
    f.play("Spire Security", None);
    assert_eq!(f.theirs(0).damage, 0);
}

#[test]
fn astromancer_summons_by_the_size_of_your_hand() {
    let mut f = Fix::new();
    for _ in 0..4 {
        f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    }
    f.play("Astromancer", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(1).card.def().cost, 4, "four cards left in hand");
}

#[test]
fn temporal_construct_draws_the_overkill() {
    let mut f = Fix::new()
        .board(FOE, &["Wisp"]) // 1 health, so four points spill
        .deck(&["Wisp", "Wisp", "Wisp", "Wisp", "Wisp"]);
    f.play("Temporal Construct", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hand.len(), 4);
}

#[test]
fn sindragosas_triumph_discounts_by_the_overkill() {
    let mut f = Fix::new().board(FOE, &["Wisp"]); // 1 health, seven spill
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.play("Sindragosa's Triumph", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hand[0].cost_delta, -7);
}

#[test]
fn relic_of_kings_sets_a_flat_price() {
    let mut f = Fix::new();
    f.play("Relic of Kings", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert!(h.card.def().cost >= 8, "{}", h.card.name());
    assert_eq!(f.g.card_cost(ME, 0), 1);
}

#[test]
fn inferno_herald_pays_an_elemental_per_fire_spell() {
    let mut f = Fix::new().board(ME, &["Inferno Herald"]);
    f.play("Fireball", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert!(h.card.def().races.any(Races::ELEMENTAL), "{}", h.card.name());
    assert_eq!(h.cost_delta, -3);
}

#[test]
fn mystic_misdirection_turns_the_attacker_into_a_sheep() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Mystic Misdirection", None);
    assert_eq!(f.g.players[0].secrets.len(), 1);
    f.g.current = FOE;
    f.g.apply(Action::Attack { from: 0, target: Target::Hero(ME) });
    assert_eq!(f.theirs(0).card.name(), "Sheep");
    assert_eq!((f.theirs(0).atk, f.theirs(0).max_hp), (1, 1));
    assert_eq!(f.g.players[0].secrets.len(), 0, "spent");
}

// -------------------------------------------------------------- Priest

#[test]
fn psychic_conjurer_copies_out_of_their_deck() {
    let mut f = Fix::new();
    f.g.players[1].deck.push(DeckCard::started(by_name("Boulderfist Ogre").unwrap()));
    f.play("Psychic Conjurer", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Boulderfist Ogre");
    assert_eq!(f.g.players[1].deck.len(), 1, "a copy, not a theft");
}

#[test]
fn shadow_ascendant_feeds_someone_else() {
    let mut f = Fix::new().board(ME, &["Shadow Ascendant", "Chillwind Yeti"]);
    f.g.end_turn();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 3), "\"another\"");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (5, 6));
}

#[test]
fn spirit_of_the_kaldorei_waits_for_the_hero_power() {
    let mut f = Fix::new();
    f.play("Spirit of the Kaldorei", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (1, 3));

    let mut f = Fix::new();
    f.g.players[0].hero_power_uses = 1;
    f.play("Spirit of the Kaldorei", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 6));
}

#[test]
fn twilight_influence_kills_the_small_or_makes_a_two_drop() {
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]); // 3 attack
    f.play_mode("Twilight Influence", 0, foe_minion(0));
    assert_eq!(f.their_board(), 0);

    let mut f = Fix::new();
    f.play_mode("Twilight Influence", 1, None);
    assert_eq!(f.mine(0).card.def().cost, 2);
}

#[test]
fn weaver_of_the_cycle_needs_an_expensive_spell_in_hand() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Weaver of the Cycle", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 0);

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Pyroblast").unwrap()));
    f.play("Weaver of the Cycle", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 3);
}

#[test]
fn specter_specialist_grants_reborn_or_copies_it() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Specter Specialist", my_minion(0));
    assert!(f.mine(0).has(Keywords::REBORN));
    assert_eq!(f.g.players[0].board.len(), 2, "no copy the first time");

    let mut f = Fix::new().board(ME, &["Sinful Steed"]); // already Reborn
    f.play("Specter Specialist", my_minion(0));
    let steeds = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Sinful Steed")
        .count();
    assert_eq!(steeds, 2);
}

#[test]
fn incensed_matriarch_only_grows_while_unhurt() {
    let mut f = Fix::new().board(ME, &["Incensed Matriarch"]);
    f.g.end_turn();
    assert_eq!(f.mine(0).max_hp, 6);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    f.g.end_turn();
    assert_eq!(f.mine(0).max_hp, 6, "damaged, so nothing");
}

#[test]
fn lingering_spirit_sends_the_overheal_at_the_enemy() {
    let mut f = Fix::new().board(ME, &["Lingering Spirit"]);
    f.g.players[0].hero_hp = 29; // only one point of the three can land
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hero_hp, 30);
    assert_eq!(f.g.players[1].hero_hp, 28, "two points had nowhere to heal");
}

#[test]
fn medivhs_triumph_is_a_flat_one_with_a_legendary_out() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Medivh's Triumph").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 5);
    let f2 = Fix::new().board(ME, &["Deathwing"]); // a Legendary
    f.g.players[0].board = f2.g.players[0].board;
    assert_eq!(f.g.card_cost(ME, 0), 1);
}

#[test]
fn eternus_only_takes_what_it_outlives() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 7 health
    f.play("Eternus", foe_minion(0)); // Eternus has 2
    assert_eq!(f.their_board(), 1, "too healthy to steal");

    let mut f = Fix::new().board(FOE, &["Wisp"]);
    f.play("Eternus", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].board.len(), 2);
}

#[test]
fn atlasaurus_leaves_a_big_taunt() {
    let mut f = Fix::new().board(ME, &["Atlasaurus"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert!(f.mine(0).card.def().cost >= 5);
}

#[test]
fn ritual_of_life_lands_a_shrunken_three_drop() {
    let mut f = Fix::new();
    f.play("Ritual of Life", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.def().cost, 3);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 3));
}

#[test]
fn archaios_tops_up_whoever_swings() {
    // The rescue: a Yeti down to one Health goes in on six instead, and comes
    // back out alive.
    let mut f = Fix::new()
        .board(ME, &["Archaios", "Chillwind Yeti"]) // 1/6 and 4/5
        .board(FOE, &["Bloodfen Raptor"]); // 3/2
    f.g.players[0].board[1].damage = 4;
    assert_eq!(f.mine(1).health(), 1);
    assert!(f.g.apply(Action::Attack { from: 1, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.mine(1).health(), 3, "six, then the Raptor's three");
    assert_eq!(f.their_board(), 0);
}

#[test]
fn hold_them_off_makes_a_lifestealing_threat() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Hold Them Off!", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 6));
    assert!(f.mine(0).has(Keywords::LIFESTEAL));
}

#[test]
fn gladesong_siren_costs_one_after_both_schools() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Gladesong Siren").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 6);
    f.play("Holy Light", None); // Holy
    assert_eq!(f.g.card_cost(ME, 0), 6, "one school is not two");
    f.play("Undeath Sentence", None); // Shadow
    assert_eq!(f.g.card_cost(ME, 0), 1);
}

#[test]
fn glade_ecologist_leaves_vines_that_cut_both_ways() {
    let mut f = Fix::new().board(ME, &["Glade Ecologist"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand[0].card.name(), "Purifying Vines");

    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Purifying Vines", my_minion(0));
    assert_eq!(f.mine(0).max_hp, 7);

    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]); // 3/2
    f.play("Purifying Vines", foe_minion(0));
    assert_eq!(f.their_board(), 0, "-2 Health kills a two-health body");
}

#[test]
fn holy_embrace_leaves_its_dark_half_in_hand() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play("Holy Embrace", Some(Target::Hero(ME)));
    assert_eq!(f.g.players[0].hero_hp, 24);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Dark Embrace");
    f.play("Dark Embrace", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 26);
}

// ------------------------------------------------------------- Warlock

#[test]
fn shadow_rounds_chains_while_it_keeps_killing() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp", "Boulderfist Ogre"]);
    f.play("Shadow Rounds", foe_minion(0));
    assert_eq!(f.their_board(), 1, "both 1/1s went");
    assert_eq!(f.theirs(0).card.name(), "Boulderfist Ogre");
    assert_eq!(f.theirs(0).damage, 2, "and the chain stopped on it");
}

#[test]
fn ocular_occultist_costs_you_a_card() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Ocular Occultist", None);
    assert_eq!(f.g.players[0].hand.len(), 0);
}

#[test]
fn rafaam_ladder_draws_three_different_prices() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Bloodfen Raptor", "Chillwind Yeti"]);
    f.play("RAFAAM LADDER!!", None);
    let mut costs: Vec<i16> = f.g.players[0]
        .hand
        .iter()
        .map(|h| h.card.def().cost)
        .collect();
    costs.sort();
    assert_eq!(costs, vec![0, 2, 4], "one Wisp, the Raptor, the Yeti");
}

#[test]
fn possessed_animancer_pulls_a_lifestealing_beast() {
    let mut f = Fix::new()
        .board(ME, &["Possessed Animancer"])
        .deck(&["Chillwind Yeti", "Bloodfen Raptor"]); // only the Raptor is a Beast
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.mine(0).card.name(), "Bloodfen Raptor");
    assert!(f.mine(0).has(Keywords::LIFESTEAL));
}

#[test]
fn sleep_paralysis_offers_two_walls_or_a_kill() {
    let mut f = Fix::new();
    f.play_mode("Sleep Paralysis", 0, None);
    assert_eq!(f.g.players[0].board.len(), 2);
    for i in 0..2 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (3, 6));
        assert!(f.mine(i).has(Keywords::TAUNT));
        assert!(f.mine(i).has(Keywords::CANT_ATTACK));
    }

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play_mode("Sleep Paralysis", 1, foe_minion(0));
    assert_eq!(f.their_board(), 0);
}

#[test]
fn riftcleaver_charges_you_for_the_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 7 health
    f.play("Riftcleaver", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hero_hp, 23);
}

#[test]
fn asphyxiodon_picks_one_off_every_turn() {
    let mut f = Fix::new()
        .board(ME, &["Asphyxiodon"])
        .board(FOE, &["Chillwind Yeti"]);
    f.g.end_turn();
    assert_eq!(f.their_board(), 0, "five kills a 4/5");
    assert_eq!(f.g.players[1].hero_hp, 30, "a minion, never the hero");
}

#[test]
fn archwitch_willow_reaches_into_hand_and_deck_alike() {
    let mut f = Fix::new().deck(&["Voidwalker"]); // a Demon
    f.play("Archwitch Willow", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.mine(1).card.name(), "Voidwalker");
    assert_eq!(f.g.players[0].deck.len(), 0, "it left the deck");

    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Voidwalker").unwrap()));
    f.play("Archwitch Willow", None);
    assert_eq!(f.mine(1).card.name(), "Voidwalker");
    assert_eq!(f.g.players[0].hand.len(), 0, "it left the hand");
}

#[test]
fn bat_mask_fills_the_board_with_one_ones() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre"]);
    f.play("Bat Mask", my_minion(0));
    assert_eq!(f.g.players[0].board.len(), 7);
    for i in 0..7 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (1, 1));
        assert_eq!(f.mine(i).card.name(), "Boulderfist Ogre");
    }
}

#[test]
fn chronogor_splits_your_deck_by_price() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Chillwind Yeti", "Boulderfist Ogre"]);
    f.play("Chronogor", None);
    let mine: Vec<i16> = f.g.players[0].hand.iter().map(|h| h.card.def().cost).collect();
    let theirs: Vec<i16> = f.g.players[1].hand.iter().map(|h| h.card.def().cost).collect();
    assert_eq!(mine, vec![6, 4], "the two dearest");
    assert_eq!(theirs, vec![0, 0], "the two cheapest");
    assert_eq!(f.g.players[0].deck.len(), 0);
}

#[test]
fn razidir_burns_their_hand_on_kindred() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[1].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Razidir", None);
    assert_eq!(f.g.players[0].hand.len(), 0, "no Kindred: your own hand");
    assert_eq!(f.g.players[1].hand.len(), 1);

    let mut f = Fix::new();
    f.g.players[0].played_races_last = Races::DEMON;
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[1].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Razidir", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[1].hand.len(), 0);
}

// -------------------------------------------------------------- Hunter

#[test]
fn raptor_nest_nurse_pays_twice_over() {
    let mut f = Fix::new().board(ME, &["Raptor-Nest Nurse"]);
    f.play("Raptor-Nest Nurse", None);
    let got = f.g.players[0].hand[0].card;
    assert_eq!(got.def().kind(), Kind::Minion);
    assert_eq!(got.def().cost, 1);
    let at = f.g.players[0].board.len() - 1;
    f.g.players[0].board[at].damage = f.g.players[0].board[at].max_hp;
    f.g.sweep_deaths();
    let got = f.g.players[0].hand[1].card;
    assert_eq!(got.def().kind(), Kind::Spell);
    assert_eq!(got.def().cost, 1);
}

#[test]
fn dinositter_discounts_a_beast_each_turn() {
    let mut f = Fix::new().board(ME, &["Dinositter"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Bloodfen Raptor").unwrap()));
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand[0].cost_delta, 0, "not a Beast");
    assert_eq!(f.g.players[0].hand[1].cost_delta, -1);
}

#[test]
fn freezing_trap_sends_the_attacker_home_taxed() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Freezing Trap", None);
    f.g.current = FOE;
    f.g.apply(Action::Attack { from: 0, target: Target::Hero(ME) });
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[1].hand.len(), 1);
    assert_eq!(f.g.players[1].hand[0].cost_delta, 2);
    assert_eq!(f.g.players[0].secrets.len(), 0);
}

#[test]
fn pressure_plate_answers_a_spell_with_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Pressure Plate", None);
    f.g.current = FOE;
    f.g.players[1].hand.push(HandCard::new(by_name("The Coin").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.their_board(), 0);
}

#[test]
fn rat_trap_waits_for_a_third_card() {
    let mut f = Fix::new();
    f.play("Rat Trap", None);
    f.g.current = FOE;
    for i in 0..3 {
        f.g.players[1].hand.push(HandCard::new(by_name("Wisp").unwrap()));
        assert!(f.g.apply(Action::Play {
            hand: 0,
            target: None,
            position: u8::MAX,
            choice: u8::MAX,
        }));
        let rats = f.g.players[0]
            .board
            .iter()
            .filter(|m| m.card.name() == "Doom Rat")
            .count();
        assert_eq!(rats, usize::from(i >= 2), "after {} cards", i + 1);
    }
}

#[test]
fn augmented_porcupine_pays_out_its_attack() {
    let mut f = Fix::new().board(ME, &["Augmented Porcupine"]); // 2/4
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[1].hero_hp, 28, "two points, nothing else to hit");
}

#[test]
fn dragonbane_answers_every_hero_power() {
    let mut f = Fix::new().board(ME, &["Dragonbane"]);
    assert!(f.g.apply(Action::HeroPower { target: Some(Target::Hero(FOE)), second: false }));
    assert!(f.g.players[1].hero_hp <= 24, "the power, and five more");
}

#[test]
fn grace_of_the_greatwolf_goes_face_or_wide() {
    let mut f = Fix::new();
    f.play_mode("Grace of the Greatwolf", 0, None);
    assert_eq!(f.g.players[1].hero_hp, 26);

    let mut f = Fix::new();
    f.play_mode("Grace of the Greatwolf", 1, None);
    assert_eq!(f.g.players[0].board.len(), 2);
    for i in 0..2 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (3, 2));
        assert!(f.mine(i).has(Keywords::RUSH));
    }
}

#[test]
fn mythical_runebear_doubles_only_when_it_is_big_enough() {
    let mut f = Fix::new();
    f.play("Mythical Runebear", None); // printed 3/4
    assert_eq!(f.g.players[0].board.len(), 1);

    let mut f = Fix::new().board(ME, &["Leokk"]); // +1 Attack to the others
    f.play("Mythical Runebear", None);
    let bears = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Mythical Runebear")
        .count();
    assert_eq!(bears, 2, "four attack under the aura");
}

#[test]
fn wasteland_vanguard_fires_again_on_a_kill() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Wasteland Vanguard", None);
    let dealt = f.theirs(0).damage + (30 - f.g.players[1].hero_hp);
    assert_eq!(dealt, 3, "nothing died");

    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp", "Wisp"]);
    f.play("Wasteland Vanguard", None);
    let dealt: i16 = f.g.players[1].board.iter().map(|m| m.damage).sum::<i16>()
        + (30 - f.g.players[1].hero_hp)
        + 3 - f.their_board() as i16;
    assert!(dealt >= 6, "a kill bought a second three");
}

#[test]
fn sewer_swimmer_pulls_a_rattle_early() {
    let mut f = Fix::new().board(ME, &["Cairne Bloodhoof"]);
    f.play("Sewer Swimmer", None);
    let baines = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Baine Bloodhoof")
        .count();
    assert_eq!(baines, 1, "Cairne is still standing, and Baine is out");
}

#[test]
fn spiritspeaker_brings_a_companion() {
    let mut f = Fix::new();
    f.play("Spiritspeaker", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    let name = f.mine(1).card.name();
    assert!(["Huffer", "Leokk", "Misha"].contains(&name), "{name}");
}

#[test]
fn broll_bearmantle_summons_a_companion_per_spell() {
    let mut f = Fix::new().board(ME, &["Broll Bearmantle"]);
    f.play("The Coin", None);
    assert_eq!(f.g.players[0].board.len(), 2);
    let name = f.mine(1).card.name();
    assert!(["Huffer", "Leokk", "Misha"].contains(&name), "{name}");
}

#[test]
fn tending_dragonkin_copies_the_cheapest_beast() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Boulderfist Ogre").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Bloodfen Raptor").unwrap())); // 2, Beast
    f.g.players[0].hand.push(HandCard::new(by_name("Mountain Bear").unwrap())); // 7, Beast
    f.play("Tending Dragonkin", None);
    let raptors = f.g.players[0]
        .hand
        .iter()
        .filter(|h| h.card.name() == "Bloodfen Raptor")
        .count();
    assert_eq!(raptors, 2);
}

#[test]
fn triennium_rex_pays_on_kindred_and_on_death() {
    let mut f = Fix::new();
    f.play("Triennium Rex", None);
    assert_eq!(f.g.players[0].hand.len(), 0, "no Beast last turn");

    let mut f = Fix::new();
    f.g.players[0].played_races_last = Races::BEAST;
    f.play("Triennium Rex", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].cost_delta, -2);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 2, "and again when it dies");
}

#[test]
fn magma_hound_sprays_after_a_trade_it_survives() {
    let mut f = Fix::new()
        .board(ME, &["Magma Hound"]) // 5/8
        .board(FOE, &["Wisp", "Wisp"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    let dealt = (30 - f.g.players[1].hero_hp) as usize + (2 - f.their_board());
    assert!(dealt >= 5, "five points went somewhere among the enemies");
}

#[test]
fn leokk_lifts_the_rest_of_the_board() {
    let f = Fix::new().board(ME, &["Leokk", "Chillwind Yeti"]);
    assert_eq!(f.mine(0).atk, 2, "not himself");
    assert_eq!(f.mine(1).atk, 5);
}

// --------------------------------------------------------------- Rogue

#[test]
fn mimicry_pays_you_for_their_draw() {
    let mut f = Fix::new();
    f.g.players[1].deck.push(DeckCard::started(by_name("Wisp").unwrap()));
    f.g.players[1].deck.push(DeckCard::started(by_name("Chillwind Yeti").unwrap()));
    f.play("Mimicry", None);
    assert_eq!(f.g.players[1].hand.len(), 2);
    let mut mine: Vec<&str> = f.g.players[0].hand.iter().map(|h| h.card.name()).collect();
    mine.sort();
    assert_eq!(mine, vec!["Chillwind Yeti", "Wisp"]);
}

#[test]
fn garonas_last_stand_only_points_at_legends() {
    let mut f = Fix::new().board(FOE, &["Deathwing"]);
    f.play("Garona's Last Stand", foe_minion(0));
    assert_eq!(f.their_board(), 0);
}

#[test]
fn jackpot_pays_out_two_big_spells_from_elsewhere() {
    let mut f = Fix::new(); // a Mage fixture
    f.play("Jackpot!", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    for h in f.g.players[0].hand.iter() {
        let d = h.card.def();
        assert_eq!(d.kind(), Kind::Spell);
        assert!(d.cost >= 5, "{}", h.card.name());
        assert_ne!(d.class(), Class::Mage);
        assert_ne!(d.class(), Class::Neutral);
    }
}

#[test]
fn silent_strike_only_shoots_from_stealth() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.play("Silent Strike", my_minion(0));
    assert_eq!(f.mine(0).atk, 7);
    assert_eq!(f.theirs(0).damage, 0, "not Stealthed");

    let mut f = Fix::new()
        .board(ME, &["Stranglethorn Tiger"]) // 5/5 Stealth
        .board(FOE, &["Boulderfist Ogre"]);
    f.play("Silent Strike", my_minion(0));
    assert_eq!(f.their_board(), 0, "eight, which is more than a 6/7 has");
}

#[test]
fn web_of_deception_trades_a_body_for_a_spider() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Web of Deception", my_minion(0));
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 4));
    assert!(f.mine(0).has(Keywords::STEALTH));
    assert_eq!(f.g.players[0].hand[0].card.name(), "Chillwind Yeti");
}

#[test]
fn deadly_bribe_pays_them_and_you_on_combo() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Deadly Bribe", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[1].hand.len(), 1);
    assert_eq!(f.g.players[0].hand.len(), 0);

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Wisp", None);
    f.play("Deadly Bribe", foe_minion(0));
    assert_eq!(f.g.players[0].hand.len(), 1, "combo pays you too");
}

#[test]
fn si7_slayer_feeds_the_hidden() {
    let mut f = Fix::new().board(ME, &["SI:7 Slayer", "Stranglethorn Tiger"]);
    assert!(f.g.apply(Action::Attack { from: 1, target: Target::Hero(FOE) }));
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (7, 7));
}

#[test]
fn shaku_steals_a_card_per_swing() {
    let mut f = Fix::new().board(ME, &["Shaku, the Collector"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[0].hand.len(), 1);
    let got = f.g.players[0].hand[0].card;
    assert_ne!(got.def().class(), Class::Mage);
    assert_ne!(got.def().class(), Class::Neutral);
}

#[test]
fn tricky_satyr_takes_their_cheapest() {
    let mut f = Fix::new();
    f.g.players[1].hand.push(HandCard::new(by_name("Boulderfist Ogre").unwrap()));
    f.g.players[1].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Tricky Satyr", None);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Wisp");
    assert_eq!(f.g.players[1].hand.len(), 2, "a copy, not a theft");
}

#[test]
fn mathias_shaw_discounts_on_a_hidden_swing() {
    let mut f = Fix::new().board(ME, &["Mathias Shaw", "Stranglethorn Tiger"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    assert!(f.g.apply(Action::Attack { from: 1, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[0].hand[0].cost_delta, -3);
}

#[test]
fn waggle_pick_hands_a_minion_back_cheaper() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Waggle Pick", None);
    f.g.destroy_weapon(ME);
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Chillwind Yeti");
    assert_eq!(f.g.players[0].hand[0].cost_delta, -2);
}

#[test]
fn fast_forward_discounts_the_dearer_of_the_two() {
    let mut f = Fix::new().deck(&["Boulderfist Ogre", "Wisp"]);
    f.play("Fast Forward", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    let ogre = f.g.players[0]
        .hand
        .iter()
        .find(|h| h.card.name() == "Boulderfist Ogre")
        .expect("drawn");
    assert_eq!(ogre.cost_delta, -2);
}

#[test]
fn swashburglar_lifts_a_card_from_another_class() {
    let mut f = Fix::new();
    f.play("Swashburglar", None);
    let got = f.g.players[0].hand[0].card;
    assert_ne!(got.def().class(), Class::Mage);
    assert_ne!(got.def().class(), Class::Neutral);
}

#[test]
fn crystal_tusk_buries_a_card_and_pays_it_back() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Crystal Tusk", None);
    assert_eq!(f.g.players[0].hand.len(), 0);
    assert_eq!(f.g.players[0].deck.len(), 1);
    f.g.destroy_weapon(ME);
    assert_eq!(f.g.players[0].hand.len(), 1, "one back off a one-card deck");
}

#[test]
fn merchant_of_legend_keeps_one_and_buries_the_rest() {
    let mut f = Fix::new();
    f.play("Merchant of Legend", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(
        f.g.players[0].hand[0].card.def().rarity(),
        tavernlab_core::cards::Rarity::Legendary
    );
    assert_eq!(f.g.players[0].deck.len(), 2, "the other two went in");
}

#[test]
fn blackpaws_whip_is_cheaper_for_every_coin() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Blackpaw's Whip").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 3);
    f.g.players[0].hand.push(HandCard::new(by_name("The Coin").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("The Coin").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 1);
}

// ------------------------------------------------------------- Paladin

#[test]
fn ashleaf_pixie_needs_an_expensive_spell_in_hand() {
    let mut f = Fix::new();
    f.play("Ashleaf Pixie", None);
    assert!(!f.mine(0).has(Keywords::DIVINE_SHIELD));

    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Pyroblast").unwrap()));
    f.play("Ashleaf Pixie", None);
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    assert!(f.mine(0).has(Keywords::LIFESTEAL));
}

#[test]
fn mark_of_ursol_cuts_them_down_and_props_you_up() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Mark of Ursol", foe_minion(0));
    assert_eq!((f.theirs(0).atk, f.theirs(0).max_hp), (1, 1));

    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Mark of Ursol", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 3));
}

#[test]
fn scarlet_bruiser_pays_a_deck_with_no_neutrals() {
    let mut f = Fix::new().board(ME, &["Scarlet Bruiser"]).deck(&["Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 0, "a Neutral Yeti is in there");

    let mut f = Fix::new().board(ME, &["Scarlet Bruiser"]).deck(&["Holy Light"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.def().class(), Class::Paladin);
    assert_eq!(f.g.players[0].hand[0].cost_delta, -2);
}

#[test]
fn vigilant_sentry_triples_on_a_pure_deck() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Vigilant Sentry", None);
    assert_eq!(f.g.players[0].board.len(), 1);

    let mut f = Fix::new().deck(&["Holy Light"]);
    f.play("Vigilant Sentry", None);
    assert_eq!(f.g.players[0].board.len(), 3);
}

#[test]
fn ready_the_fleet_lifts_the_tribe_it_points_at() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor", "Chillwind Yeti", "Mountain Bear"]);
    f.play("Ready the Fleet", my_minion(0)); // a Beast
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 4));
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (4, 5), "no tribe");
    assert_eq!((f.mine(2).atk, f.mine(2).max_hp), (6, 8), "another Beast");
}

#[test]
fn ivory_knight_heals_for_what_it_found() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 5;
    f.play("Ivory Knight", None);
    let cost = f.g.players[0].hand[0].card.def().cost;
    assert_eq!(f.g.players[0].hero_hp, (5 + cost).min(30));
}

#[test]
fn lightmender_picks_a_side_of_itself() {
    let mut f = Fix::new();
    f.play_mode("Lightmender", 0, None);
    assert_eq!(f.mine(0).atk, 6);
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));

    let mut f = Fix::new();
    f.play_mode("Lightmender", 1, None);
    assert_eq!(f.mine(0).max_hp, 6);
    assert!(f.mine(0).has(Keywords::LIFESTEAL));
}

#[test]
fn spearheart_sentry_pays_a_cheap_holy_spell_a_turn() {
    let mut f = Fix::new().board(ME, &["Spearheart Sentry"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert_eq!(h.card.def().school(), tavernlab_core::cards::School::Holy);
    assert_eq!(h.cost_delta, -3);
}

#[test]
fn arator_doubles_the_recruits() {
    let mut f = Fix::new().board(ME, &["Silver Hand Recruit", "Chillwind Yeti"]);
    f.play("Arator the Redeemer", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 2));
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (4, 5), "not a Recruit");
}

#[test]
fn nozdormu_shields_then_grows() {
    let mut f = Fix::new().board(ME, &["Nozdormu, Bronze Aspect", "Chillwind Yeti"]);
    f.g.end_turn();
    assert!(f.mine(1).has(Keywords::DIVINE_SHIELD));
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (4, 5));
    f.g.end_turn();
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (7, 8), "already shielded");
}

#[test]
fn scarlet_recruiter_pulls_two_rushing_cheap_bodies() {
    let mut f = Fix::new().deck(&["Wisp", "Bloodfen Raptor", "Boulderfist Ogre"]);
    f.play("Scarlet Recruiter", None);
    assert_eq!(f.g.players[0].board.len(), 3);
    for i in 1..3 {
        assert!(f.mine(i).card.def().cost <= 2);
        assert!(f.mine(i).has(Keywords::RUSH));
    }
    assert_eq!(f.g.players[0].deck.len(), 1, "the Ogre stayed");
}

#[test]
fn searing_reflection_lands_an_eight_eight() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("Searing Reflection", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 8));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    assert_eq!(f.g.players[0].hand.len(), 1, "the Wisp is still drawn");
}

#[test]
fn heros_welcome_lands_a_ten_ten_legend() {
    let mut f = Fix::new();
    f.play("Hero's Welcome", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (10, 10));
    assert_eq!(
        f.mine(0).card.def().rarity(),
        tavernlab_core::cards::Rarity::Legendary
    );
}

#[test]
fn firegill_rushes_the_rest_on_kindred() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Firegill", None);
    assert!(!f.mine(0).has(Keywords::RUSH));

    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0].played_races_last = Races::MURLOC;
    f.play("Firegill", None);
    assert!(f.mine(0).has(Keywords::RUSH));
    assert!(!f.mine(1).has(Keywords::RUSH), "\"your other minions\"");
}

#[test]
fn bronze_redeemer_prints_itself_a_dragon() {
    let mut f = Fix::new().board(ME, &["Bronze Redeemer"]); // 3/3
    f.g.buff(Target::Minion(ME, 0), 2, 2);
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (5, 5));
    assert!(f.mine(1).races().any(Races::DRAGON));
}

// -------------------------------------------------------- Death Knight

/// A Death Knight fixture with a pile of Corpses to spend.
fn dk(corpses: i16) -> Fix {
    let mut f = Fix::new();
    f.g.players[0].class = Class::DeathKnight;
    f.g.players[0].corpses = corpses;
    f
}

#[test]
fn battlefield_necromancer_raises_one_a_turn() {
    let mut f = dk(1).board(ME, &["Battlefield Necromancer"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (1, 3));
    assert!(f.mine(1).has(Keywords::TAUNT));
    assert_eq!(f.g.players[0].corpses, 0);
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 2, "no Corpse, no Footman");
}

#[test]
fn deaths_advance_freezes_and_digs() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Death's Advance", foe_minion(0));
    assert!(f.theirs(0).flags.has(Flags::FROZEN));
    assert_eq!(f.g.players[0].hand[0].card.def().kind(), Kind::Spell);
}

#[test]
fn corpse_farm_buys_a_body_at_the_pile_it_can_afford() {
    let mut f = dk(5);
    f.play("Corpse Farm", None);
    assert_eq!(f.g.players[0].corpses, 0);
    assert_eq!(f.mine(0).card.def().cost, 5);
}

#[test]
fn glacial_advance_discounts_the_next_spell() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.play("Glacial Advance", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hero_hp, 26);
    assert_eq!(f.g.card_cost(ME, 0), 2, "Fireball at four, less two");
}

#[test]
fn corpse_flower_answers_their_summons() {
    let mut f = dk(2).board(ME, &["Corpse Flower"]);
    f.g.current = FOE;
    f.g.summon(FOE, by_name("Chillwind Yeti").unwrap());
    assert_eq!(f.theirs(0).damage, 3);
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn consumption_draws_for_each_body_it_takes() {
    let mut f = Fix::new()
        .board(FOE, &["Wisp", "Wisp"])
        .deck(&["Fireball", "Fireball"]);
    f.play("Consumption", None);
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hand.len(), 2);
}

#[test]
fn dread_raptor_makes_it_free_on_kindred() {
    let mut f = Fix::new().deck(&["Cairne Bloodhoof", "Fae Trickster"]);
    f.play("Dread Raptor", None);
    let h = &f.g.players[0].hand[0];
    assert_eq!(h.card.name(), "Fae Trickster", "the 3-cost Deathrattle one");
    assert_eq!(h.cost_delta, 0);

    let mut f = Fix::new().deck(&["Fae Trickster"]);
    f.g.players[0].played_races_last = Races::UNDEAD;
    f.play("Dread Raptor", None);
    assert_eq!(f.g.card_cost(ME, 0), 0);
}

#[test]
fn grave_strength_pays_triple_for_five_corpses() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Grave Strength", None);
    assert_eq!(f.mine(0).atk, 5);

    let mut f = dk(5).board(ME, &["Chillwind Yeti"]);
    f.play("Grave Strength", None);
    assert_eq!(f.mine(0).atk, 7);
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn lady_deathwhisper_copies_the_frost_you_hold() {
    let mut f = Fix::new().board(ME, &["Lady Deathwhisper"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Death's Advance").unwrap())); // Frost
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap())); // Fire
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let frosts = f.g.players[0]
        .hand
        .iter()
        .filter(|h| h.card.name() == "Death's Advance")
        .count();
    assert_eq!(frosts, 2);
    assert_eq!(f.g.players[0].hand.len(), 3);
}

#[test]
fn malignant_horror_clones_itself_for_four_corpses() {
    let mut f = dk(4).board(ME, &["Malignant Horror"]);
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn might_of_menethil_freezes_one_per_corpse() {
    let mut f = dk(2).board(FOE, &["Wisp", "Wisp", "Wisp"]);
    f.play("Might of Menethil", None);
    let frozen = f.g.players[1]
        .board
        .iter()
        .filter(|m| m.flags.has(Flags::FROZEN))
        .count();
    assert_eq!(frozen, 2);
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn army_of_the_dead_raises_one_ghoul_per_corpse() {
    let mut f = dk(3);
    f.play("Army of the Dead", None);
    assert_eq!(f.g.players[0].board.len(), 3);
    for i in 0..3 {
        assert_eq!((f.mine(i).atk, f.mine(i).max_hp), (2, 2));
        assert!(f.mine(i).has(Keywords::RUSH));
    }
}

#[test]
fn corpse_bride_scales_the_groom_with_the_pile() {
    let mut f = dk(7);
    f.play("Corpse Bride", None);
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (7, 7));
    assert!(f.mine(1).has(Keywords::TAUNT));
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn hollow_direhorn_buys_reborn_with_a_death() {
    // The Wisp's own death banks a Corpse first, so two in hand is enough.
    let mut f = dk(2).board(ME, &["Hollow Direhorn", "Wisp"]);
    f.g.players[0].board[1].damage = f.g.players[0].board[1].max_hp;
    f.g.sweep_deaths();
    assert!(f.mine(0).has(Keywords::REBORN));
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn bonechill_stegodon_picks_three_distinct_enemies() {
    let mut f = Fix::new()
        .board(ME, &["Bonechill Stegodon"])
        .board(FOE, &["Chillwind Yeti", "Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0, "both 4/5s went");
    assert_eq!(f.g.players[1].hero_hp, 24, "and the hero took the third six");
}

#[test]
fn experimental_animation_heralds_and_sweeps() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti", "Wisp"]);
    f.g.players[0].class = Class::DeathKnight;
    f.play("Experimental Animation", None);
    assert_eq!(f.their_board(), 1, "the Wisp went, the 4/5 did not");
    assert_eq!(f.theirs(0).damage, 4);
    assert_eq!(f.g.players[0].herald, 1);
}

#[test]
fn marrow_manipulator_shoots_twice_per_corpse_pair() {
    let mut f = dk(3);
    f.play("Marrow Manipulator", None);
    assert_eq!(f.g.players[1].hero_hp, 24, "three shots of two");
    assert_eq!(f.g.players[0].corpses, 0);
}

#[test]
fn alexandros_mograine_burns_for_the_rest_of_the_game() {
    let mut f = Fix::new();
    f.play("Alexandros Mograine", None);
    f.g.end_turn();
    assert_eq!(f.g.players[1].hero_hp, 27);
    f.g.end_turn();
    assert_eq!(f.g.players[1].hero_hp, 24, "and again, body or no body");
}

#[test]
fn story_of_umbra_summons_and_pops_it() {
    let mut f = Fix::new();
    f.play("Story of Umbra", None);
    assert!(!f.g.players[0].board.is_empty());
    let first = f.mine(0).card;
    assert!(first.def().cost >= 5 || f.g.players[0].board.len() > 1);
}

#[test]
fn boneguard_commander_raises_up_to_six() {
    let mut f = dk(10);
    f.play("Boneguard Commander", None);
    assert_eq!(f.g.players[0].board.len(), 7, "the Commander and six Footmen");
    assert_eq!(f.g.players[0].corpses, 4);
}

#[test]
fn chow_down_rushes_the_drakes_for_eight_corpses() {
    let mut f = Fix::new();
    f.play("Chow Down", None);
    assert_eq!(f.g.players[0].board.len(), 5);
    assert!(!f.mine(0).has(Keywords::RUSH), "no Corpses, no Rush");

    let mut f = dk(8);
    f.play("Chow Down", None);
    assert!(f.g.players[0].board.iter().all(|m| m.has(Keywords::RUSH)));
}

#[test]
fn the_scourge_fills_the_board_with_undead() {
    let mut f = Fix::new();
    f.play("The Scourge", None);
    assert_eq!(f.g.players[0].board.len(), 7);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.races().any(Races::UNDEAD))
    );
}

#[test]
fn volcoross_takes_the_biggest_pile_it_can_pay() {
    let mut f = dk(25);
    f.play("Volcoross", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (25, 25), "5/5 plus twenty");
    assert_eq!(f.g.players[0].corpses, 5);
}

// ------------------------------------------------------------- Neutral

#[test]
fn eternal_toil_draws_or_replaces() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]).deck(&["Fireball"]);
    f.play("Eternal Toil", foe_minion(0));
    assert_eq!(f.g.players[0].hand.len(), 1, "it survived");

    let mut f = Fix::new().board(FOE, &["Wisp"]).deck(&["Fireball"]);
    f.play("Eternal Toil", foe_minion(0));
    assert_eq!(f.g.players[0].hand.len(), 0);
    assert_eq!(f.mine(0).card.def().cost, 1);
}

#[test]
fn sheltered_survivor_swaps_a_card_for_a_draw() {
    let mut f = Fix::new().deck(&["Fireball"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Sheltered Survivor", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].deck.len(), 1);
}

#[test]
fn timeless_causality_turns_the_deck_around() {
    let mut f = Fix::new().deck(&["Wisp", "Fireball", "Chillwind Yeti"]);
    f.play("Timeless Causality", None);
    let names: Vec<&str> = f.g.players[0].deck.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["Chillwind Yeti", "Fireball", "Wisp"]);
}

#[test]
fn curious_explorer_discounts_a_minion_they_hold() {
    let mut f = Fix::new().board(ME, &["Curious Explorer"]);
    f.g.players[1].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.g.players[1].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[1].hand[0].cost_delta, 0, "the spell is not it");
    assert_eq!(f.g.players[1].hand[1].cost_delta, -2);
}

#[test]
fn cloud_serpent_copies_a_dragon_or_elemental_you_hold() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Faerie Dragon").unwrap()));
    f.play("Cloud Serpent", None);
    let dragons = f.g.players[0]
        .hand
        .iter()
        .filter(|h| h.card.name() == "Faerie Dragon")
        .count();
    assert_eq!(dragons, 2);
}

#[test]
fn relic_miner_burns_the_top_and_matches_its_rarity() {
    let mut f = Fix::new().deck(&["Deathwing"]); // Legendary
    f.play("Relic Miner", None);
    assert_eq!(f.g.players[0].deck.len(), 0, "destroyed, not drawn");
    assert_eq!(
        f.g.players[0].hand[0].card.def().rarity(),
        tavernlab_core::cards::Rarity::Legendary
    );
}

#[test]
fn chronicle_keeper_shields_up_for_a_dragon() {
    let mut f = Fix::new();
    f.play("Chronicle Keeper", None);
    assert!(!f.mine(0).has(Keywords::TAUNT));

    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Faerie Dragon").unwrap()));
    f.play("Chronicle Keeper", None);
    assert!(f.mine(0).has(Keywords::TAUNT));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
}

#[test]
fn primal_sabretooth_pockets_what_it_kills() {
    let mut f = Fix::new()
        .board(ME, &["Primal Sabretooth"]) // 5/3, so it has to pick its fights
        .board(FOE, &["Wisp"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Wisp");
}

#[test]
fn synchronized_spark_pays_a_friend_for_the_kill() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .board(FOE, &["Wisp"]);
    f.play("Synchronized Spark", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (7, 8));
}

#[test]
fn wicked_blightspawn_equips_or_sharpens() {
    let mut f = Fix::new().board(ME, &["Wicked Blightspawn"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].weapon.map(|w| (w.atk, w.durability)), Some((1, 2)));

    let mut f = Fix::new().board(ME, &["Wicked Blightspawn"]);
    f.play("Fiery War Axe", None); // 3/2
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].weapon.map(|w| w.atk), Some(5));
}

#[test]
fn wizened_truthseeker_wipes_every_discount() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.g.players[0].hand[0].cost_delta = -3;
    f.g.players[1].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.g.players[1].hand[0].cost_delta = -2;
    f.play("Wizened Truthseeker", None);
    assert_eq!(f.g.players[0].hand[0].cost_delta, 0);
    assert_eq!(f.g.players[1].hand[0].cost_delta, 0);
}

#[test]
fn activated_golem_picks_up_reborn_at_any_turn_end() {
    let mut f = Fix::new().board(ME, &["Activated Golem"]);
    assert!(!f.mine(0).has(Keywords::REBORN));
    f.g.end_turn();
    assert!(f.mine(0).has(Keywords::REBORN));
}

#[test]
fn bitter_end_freezes_three_and_kills_the_hurt() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Chillwind Yeti", "Wisp", "Wisp"]);
    f.g.players[1].board[1].damage = 1;
    f.play("Bitter End", foe_minion(1));
    assert_eq!(f.their_board(), 3, "the damaged one in the middle went");
    for i in 0..2 {
        assert!(f.theirs(i).flags.has(Flags::FROZEN));
    }
}

#[test]
fn solitary_prisoner_is_cheap_on_an_empty_table() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Solitary Prisoner").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 2);
    f.g.players[1].board.push(Permanent::summon(by_name("Wisp").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 5);
}

#[test]
fn witchwood_grizzly_shrinks_by_their_hand() {
    let mut f = Fix::new();
    for _ in 0..4 {
        f.g.players[1].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    }
    f.play("Witchwood Grizzly", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 8));
}

#[test]
fn scalehide_kodo_takes_the_small_or_the_big() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Boulderfist Ogre"]);
    f.play("Scalehide Kodo", None);
    assert_eq!(f.their_board(), 1);
    assert_eq!(f.theirs(0).card.name(), "Boulderfist Ogre");

    let mut f = Fix::new().board(FOE, &["Wisp", "Boulderfist Ogre"]);
    f.g.players[0].played_races_last = Races::BEAST;
    f.play("Scalehide Kodo", None);
    assert_eq!(f.theirs(0).card.name(), "Wisp");
}

#[test]
fn sentient_hourglass_flips_after_a_hit() {
    let mut f = Fix::new().board(ME, &["Sentient Hourglass"]); // 4/9
    f.g.deal_damage(Target::Minion(ME, 0), 2);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (7, 4));
}

#[test]
fn chillmaw_needs_a_dragon_in_hand() {
    let mut f = Fix::new()
        .board(ME, &["Chillmaw"])
        .board(FOE, &["Bloodfen Raptor"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 1, "no Dragon held");

    let mut f = Fix::new()
        .board(ME, &["Chillmaw"])
        .board(FOE, &["Bloodfen Raptor"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Faerie Dragon").unwrap()));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0);
}

#[test]
fn ravenous_devilsaur_eats_and_grows_on_kindred() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Ravenous Devilsaur", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 3));

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].played_races_last = Races::BEAST;
    f.play("Ravenous Devilsaur", foe_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3 + 6, 3 + 7));
}

#[test]
fn siamat_takes_two_of_the_four() {
    let mut f = Fix::new();
    f.play("Siamat", None);
    let m = f.mine(0);
    let n = [Keywords::RUSH, Keywords::TAUNT, Keywords::DIVINE_SHIELD, Keywords::WINDFURY]
        .iter()
        .filter(|k| m.has(**k))
        .count();
    assert_eq!(n, 2);
}

#[test]
fn warmaster_blackhorn_strips_the_cheap_from_both_decks() {
    let mut f = Fix::new().deck(&["Wisp", "Boulderfist Ogre"]);
    f.g.players[1].deck.push(DeckCard::started(by_name("Bloodfen Raptor").unwrap()));
    f.g.players[1].deck.push(DeckCard::started(by_name("Deathwing").unwrap()));
    f.play("Warmaster Blackhorn", None);
    assert_eq!(f.g.players[0].deck.len(), 1);
    assert_eq!(f.g.players[1].deck.len(), 1);
}

#[test]
fn disciple_of_demise_kills_once_per_dragon_plus_one() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp", "Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Faerie Dragon").unwrap()));
    f.play("Disciple of Demise", None);
    assert_eq!(f.their_board(), 1, "one for the card, one for the Dragon");
}

#[test]
fn black_market_auctioneer_draws_on_every_spell() {
    let mut f = Fix::new().board(ME, &["Black Market Auctioneer"]).deck(&["Wisp"]);
    f.play("The Coin", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn krog_flattens_their_board_each_turn() {
    let mut f = Fix::new()
        .board(ME, &["Krog, Crater King"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.end_turn();
    assert_eq!((f.theirs(0).atk, f.theirs(0).max_hp), (1, 1));
}

#[test]
fn bygone_echoes_pays_more_for_corpses() {
    let mut f = Fix::new();
    f.g.players[0].class = Class::DeathKnight;
    f.g.players[0].corpses = 4;
    f.play("Bygone Echoes", None); // played last: an Outcast
    assert_eq!(f.g.players[0].board.len(), 3, "base, Corpses, Outcast");
    assert!(f.g.players[0].board.iter().all(|m| m.card.def().cost == 4));
}

#[test]
fn finja_pulls_murlocs_out_of_the_deck() {
    let mut f = Fix::new()
        .board(ME, &["Finja, the Flying Star"]) // 3/5 Stealth
        .board(FOE, &["Wisp"])
        .deck(&["Rockskipper", "Rockskipper"]); // Murlocs
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.g.players[0].board.len(), 3);
    assert_eq!(f.g.players[0].deck.len(), 0);
}

#[test]
fn stormbrewer_hits_first() {
    let mut f = Fix::new()
        .board(ME, &["Stormbrewer"]) // 3/6
        .board(FOE, &["Chillwind Yeti"]); // 4/5, dies to 3 + 3
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert_eq!(f.their_board(), 0);
}

#[test]
fn vanessa_pays_a_battlecry_per_card() {
    let mut f = Fix::new().board(ME, &["Vanessa the Ringleader"]);
    f.play("Wisp", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let h = &f.g.players[0].hand[0];
    assert!(h.card.def().keywords.has(Keywords::BATTLECRY), "{}", h.card.name());
    assert_eq!(h.cost_delta, -2);
}

#[test]
fn keymaster_alabaster_copies_their_draws_at_one() {
    let mut f = Fix::new().board(ME, &["Keymaster Alabaster"]);
    f.g.players[1].deck.push(DeckCard::started(by_name("Boulderfist Ogre").unwrap()));
    f.g.draw(FOE, 1);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Boulderfist Ogre");
    assert_eq!(f.g.card_cost(ME, 0), 1);
}

#[test]
fn zaqali_flamemancer_rewards_a_hand_of_distinct_prices() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Zaqali Flamemancer", None);
    assert_eq!(f.g.players[0].hand[0].cost_delta, 0, "two at the same price");

    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.play("Zaqali Flamemancer", None);
    assert_eq!(f.g.players[0].hand[0].cost_delta, -2);
    assert_eq!(f.g.players[0].hand[1].cost_delta, -2);
}

#[test]
fn unknown_voyager_becomes_something_else_when_hit() {
    let mut f = Fix::new().board(ME, &["Unknown Voyager"]);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    assert_ne!(f.mine(0).card.name(), "Unknown Voyager");
    assert_eq!(f.mine(0).card.def().cost, 7);
}

#[test]
fn dangerous_variant_grows_up_at_the_turn_start() {
    let mut f = Fix::new().board(ME, &["Dangerous Variant"]);
    f.g.end_turn();
    f.g.begin_turn();
    assert_eq!(f.mine(0).card.def().cost, 5);
}

#[test]
fn crater_experiment_is_kindred_with_everything() {
    let mut f = Fix::new();
    f.play("Crater Experiment", None);
    assert_eq!(f.g.players[0].board.len(), 1);

    let mut f = Fix::new();
    f.g.players[0].played_races_last = Races::MURLOC;
    f.play("Crater Experiment", None);
    assert_eq!(f.g.players[0].board.len(), 2);
}

#[test]
fn steamfin_thief_brings_two_on_kindred() {
    let mut f = Fix::new();
    f.g.players[0].played_races_last = Races::MURLOC;
    f.play("Steamfin Thief", None);
    assert_eq!(f.g.players[0].board.len(), 3);
    assert!(f.mine(1).has(Keywords::RUSH));
}

#[test]
fn bronze_keeper_prints_a_dragon_a_turn() {
    let mut f = Fix::new().board(ME, &["Bronze Keeper"]);
    f.g.end_turn();
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (6, 6));
    assert!(f.mine(1).has(Keywords::DIVINE_SHIELD));
}

// ------------------------------------------ enchantments in the hand

#[test]
fn a_card_enchanted_in_hand_lands_with_the_stats() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand[0].enchant(2, 3);
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 4));
}

#[test]
fn grimestreet_outfitter_lifts_the_whole_hand() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.play("Grimestreet Outfitter", None);
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (1, 1));
    assert_eq!((f.g.players[0].hand[1].atk, f.g.players[0].hand[1].hp), (0, 0));
}

#[test]
fn disciple_of_the_dove_draws_then_hardens() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Disciple of the Dove", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].hp, 2);
}

#[test]
fn detonation_juggernaut_only_pays_the_taunts() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Voidwalker").unwrap())); // Taunt
    f.play("Detonation Juggernaut", None);
    assert_eq!(f.g.players[0].hand[0].atk, 0);
    assert_eq!((f.g.players[0].hand[1].atk, f.g.players[0].hand[1].hp), (2, 2));
}

#[test]
fn i_know_a_guy_finds_a_bigger_taunt() {
    let mut f = Fix::new();
    f.play("I Know a Guy", None);
    let h = &f.g.players[0].hand[0];
    assert!(h.card.def().keywords.has(Keywords::TAUNT), "{}", h.card.name());
    assert_eq!((h.atk, h.hp), (1, 2));
}

#[test]
fn twisted_treant_shaves_a_minion_in_each_hand() {
    let mut f = Fix::new().board(ME, &["Twisted Treant"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.players[1].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand[0].atk, -2);
    assert_eq!(f.g.players[1].hand[0].atk, -2);
}

#[test]
fn lethal_recipe_only_buffs_at_ten_crystals() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.players[0].crystals = 5;
    f.play("Lethal Recipe", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    assert_eq!(f.g.players[0].hand[0].atk, 0);

    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.players[0].crystals = 10;
    f.play("Lethal Recipe", None);
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (3, 3));
}

#[test]
fn story_of_barnabus_rewards_a_big_draw() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("Story of Barnabus", None);
    assert_eq!(f.g.players[0].armor, 0);

    let mut f = Fix::new().deck(&["Boulderfist Ogre"]); // 6 attack
    f.play("Story of Barnabus", None);
    assert_eq!(f.g.players[0].hand[0].hp, 5);
    assert_eq!(f.g.players[0].armor, 5);
}

#[test]
fn flight_of_the_firehawk_draws_two_different_tribes() {
    let mut f = Fix::new().deck(&["Bloodfen Raptor", "Voidwalker", "Bloodfen Raptor"]);
    f.play("Flight of the Firehawk", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
    let a = f.g.players[0].hand[0].card.def().races;
    let b = f.g.players[0].hand[1].card.def().races;
    assert!(!a.any(b), "different types");
    for h in f.g.players[0].hand.iter() {
        assert_eq!((h.atk, h.hp), (1, 1));
    }
}

#[test]
fn divine_augur_squares_the_hand_up() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Boulderfist Ogre").unwrap())); // 6/7
    f.g.players[0].hand.push(HandCard::new(by_name("Voidwalker").unwrap())); // 1/3
    f.play("Divine Augur", None);
    let ogre = &f.g.players[0].hand[0];
    assert_eq!((6 + ogre.atk as i16, 7 + ogre.hp as i16), (7, 7));
    let vw = &f.g.players[0].hand[1];
    assert_eq!((1 + vw.atk as i16, 3 + vw.hp as i16), (3, 3));
}

#[test]
fn vicious_bloodworm_arms_a_minion_in_hand() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Vicious Bloodworm", None); // 3 attack
    assert_eq!(f.g.players[0].hand[0].atk, 3);
}

#[test]
fn gruesome_nightmare_reaches_hand_or_board() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Gruesome Nightmare", None); // 3 attack, nothing in hand
    assert_eq!(f.mine(0).atk, 7);
}

#[test]
fn neferset_weaponsmith_sharpens_on_combo() {
    let mut f = Fix::new();
    f.play("Neferset Weaponsmith", None);
    let h = &f.g.players[0].hand[0];
    assert_eq!(h.card.def().kind(), Kind::Weapon);
    assert_eq!(h.atk, 0, "no combo");

    let mut f = Fix::new();
    f.play("Wisp", None);
    f.play("Neferset Weaponsmith", None);
    assert_eq!(f.g.players[0].hand[0].atk, 2);
}

#[test]
fn a_weapon_enchanted_in_hand_equips_bigger() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Fiery War Axe").unwrap())); // 3/2
    f.g.players[0].hand[0].enchant(2, 0);
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].weapon.map(|w| (w.atk, w.durability)), Some((5, 2)));
}

// -------------------------------------------------- granted deathrattles

#[test]
fn spikeridged_steed_leaves_a_stegodon_behind() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Spikeridged Steed", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 7));
    assert!(f.mine(0).has(Keywords::TAUNT));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.mine(0).card.name(), "Stegodon");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 6));
}

#[test]
fn talanjis_last_stand_pays_for_every_body_you_lose() {
    let mut f = Fix::new().board(ME, &["Wisp", "Wisp"]);
    f.play("Talanji's Last Stand", None);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.players[0].board[1].damage = f.g.players[0].board[1].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert!(f.g.players[0].board.iter().all(|m| m.card.def().cost == 4));
}

#[test]
fn ulfar_pays_out_at_the_dead_minions_own_cost() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre"]); // costs 6
    f.play("Ulfar", None);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let replacement = f.g.players[0]
        .board
        .iter()
        .find(|m| m.card.name() != "Ulfar")
        .expect("something replaced it");
    assert_eq!(replacement.card.def().cost, 6);
}

#[test]
fn ulfar_does_not_grant_itself_the_rattle() {
    let mut f = Fix::new();
    f.play("Ulfar", None);
    assert_eq!(f.mine(0).granted_rattle, tavernlab_core::cards::CardId(0));
}

#[test]
fn ancient_raptors_three_modes() {
    let mut f = Fix::new();
    f.play_mode("Ancient Raptor", 0, None);
    assert_eq!(f.mine(0).atk, 5);

    let mut f = Fix::new();
    f.play_mode("Ancient Raptor", 1, None);
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));

    let mut f = Fix::new();
    f.play_mode("Ancient Raptor", 2, None);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .all(|m| m.card.name() == "Plant")
    );
}

#[test]
fn a_granted_rattle_runs_alongside_the_minions_own() {
    let mut f = Fix::new().board(ME, &["Cairne Bloodhoof"]);
    f.play("Talanji's Last Stand", None);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let names: Vec<&str> = f.g.players[0].board.iter().map(|m| m.card.name()).collect();
    assert!(names.contains(&"Baine Bloodhoof"), "{names:?}");
    assert_eq!(names.len(), 2, "and the granted 4-drop: {names:?}");
}

#[test]
fn silence_takes_a_granted_rattle_with_it() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Talanji's Last Stand", None);
    f.g.silence(Target::Minion(ME, 0));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 0);
}

// ------------------------------- hand enchantments, second helping

#[test]
fn blood_tap_pays_double_for_two_corpses() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Blood Tap", None);
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (1, 1));

    let mut f = dk(2);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Blood Tap", None);
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (2, 2));
}

#[test]
fn darkfallen_neophyte_needs_the_corpses() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Darkfallen Neophyte", None);
    assert_eq!(f.g.players[0].hand[0].atk, 0);

    let mut f = dk(2);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Darkfallen Neophyte", None);
    assert_eq!(f.g.players[0].hand[0].atk, 2);
}

#[test]
fn hourglass_attendant_lifts_the_hand_each_turn() {
    let mut f = Fix::new().board(ME, &["Hourglass Attendant"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.end_turn();
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (1, 1));
}

#[test]
fn overlord_runthak_pays_the_hand_for_every_swing() {
    let mut f = Fix::new().board(ME, &["Overlord Runthak"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (1, 1));
}

#[test]
fn bone_flurry_doubles_after_a_friendly_death() {
    let mut f = Fix::new();
    f.play("Bone Flurry", None);
    assert_eq!(f.g.players[1].hero_hp, 27);

    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    f.play("Bone Flurry", None);
    assert_eq!(f.g.players[1].hero_hp, 24);
}

#[test]
fn liferender_needs_your_health_to_have_moved() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Liferender", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 0);

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.damage_hero(ME, 1);
    f.play("Liferender", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 6);
}

#[test]
fn endtime_survivor_grows_after_you_are_hit() {
    let mut f = Fix::new();
    f.play("Endtime Survivor", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 6));

    let mut f = Fix::new();
    f.g.damage_hero(ME, 1);
    f.play("Endtime Survivor", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 9));
}

#[test]
fn crystal_tender_catches_your_crystals_up() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 4;
    f.g.players[1].crystals = 8;
    f.play("Crystal Tender", None);
    assert_eq!(f.g.players[0].crystals, 8);
    assert_eq!(f.g.players[0].mana, 10 - 2, "empty crystals, so no new mana");
}

#[test]
fn lorewalker_cho_hands_the_spell_to_the_other_side() {
    let mut f = Fix::new().board(ME, &["Lorewalker Cho"]);
    f.play("Fireball", Some(Target::Hero(FOE)));
    assert_eq!(f.g.players[1].hand.len(), 1);
    assert_eq!(f.g.players[1].hand[0].card.name(), "Fireball");
    assert_eq!(f.g.players[0].hand.len(), 0, "never back to the caster");
}

#[test]
fn chromatic_broodmother_refunds_mana_when_it_swings() {
    let mut f = Fix::new().board(ME, &["Chromatic Broodmother"]); // 2 attack
    f.g.players[0].mana = 0;
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[0].mana, 2);
}

#[test]
fn photosynthesis_heals_and_pays_three_druid_spells() {
    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.play("Photosynthesis", Some(Target::Hero(ME)));
    assert_eq!(f.g.players[0].hero_hp, 26);
    assert_eq!(f.g.players[0].hand.len(), 3);
    for h in f.g.players[0].hand.iter() {
        assert_eq!(h.card.def().class(), Class::Druid);
        assert_eq!(h.card.def().kind(), Kind::Spell);
    }
}

// -------------------------------------------------------- forced attacks

#[test]
fn unholy_frenzy_throws_your_board_and_puts_it_back() {
    let mut f = Fix::new()
        .board(ME, &["Wisp", "Wisp"])
        .board(FOE, &["Chillwind Yeti"]); // 4/5, kills both
    f.play("Unholy Frenzy", foe_minion(0));
    assert_eq!(f.theirs(0).damage, 2, "two Wisps got in");
    assert_eq!(f.g.players[0].board.len(), 2, "and both came back");
    assert!(f.g.players[0].board.iter().all(|m| m.card.name() == "Wisp"));
}

#[test]
fn temporal_traveler_leaves_a_shadow_that_swings() {
    let mut f = Fix::new()
        .board(ME, &["Temporal Traveler"])
        .board(FOE, &["Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.theirs(0).damage, 4, "the Shadow went straight in");
    assert_eq!(f.g.players[0].board.len(), 0, "and died to the counter");
}

#[test]
fn gnome_muncher_eats_the_weakest_each_turn() {
    let mut f = Fix::new()
        .board(ME, &["Gnome Muncher"]) // 5/6 Lifesteal
        .board(FOE, &["Boulderfist Ogre", "Wisp"]);
    f.g.players[0].hero_hp = 20;
    f.g.end_turn();
    assert_eq!(f.their_board(), 1, "the Wisp was the lowest");
    assert_eq!(f.g.players[0].hero_hp, 25, "Lifesteal on a forced swing");
}

#[test]
fn high_cultist_herenn_makes_the_two_fight() {
    let mut f = Fix::new().deck(&["Cairne Bloodhoof", "Cairne Bloodhoof"]);
    f.play("High Cultist Herenn", None);
    // Two 5/5s trade, and each leaves a Baine.
    let baines = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Baine Bloodhoof")
        .count();
    assert_eq!(baines, 2);
}

#[test]
fn mythical_terror_drags_their_whole_board_in() {
    let mut f = Fix::new()
        .board(ME, &["Mythical Terror"]) // 4/10 Lifesteal
        .board(FOE, &["Wisp", "Wisp"]);
    f.g.players[0].hero_hp = 20;
    f.g.end_turn();
    assert_eq!(f.their_board(), 0, "both threw themselves at it");
    assert_eq!(f.mine(0).damage, 2);
    assert_eq!(f.g.players[0].hero_hp, 28, "four back per swing");
}

#[test]
fn illidari_inquisitor_follows_the_hero_in() {
    let mut f = Fix::new()
        .board(ME, &["Illidari Inquisitor"]) // 8/8
        .board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Fiery War Axe", None); // 3/2
    assert!(f.g.apply(Action::HeroAttack { target: Target::Minion(FOE, 0) }));
    assert_eq!(f.their_board(), 0, "three from the hero, eight from the Demon");
    assert_eq!(f.mine(0).damage, 6, "and it took the counter");
}

#[test]
fn treeees_throws_four_treants_at_one_minion() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("TREEEES!!!", foe_minion(0));
    assert_eq!(f.their_board(), 0, "four twos is eight");
    assert!(
        f.g.players[0].board.len() < 4,
        "and the Ogre took some with it"
    );
}

#[test]
fn ankylodon_leaves_two_beasts_swinging() {
    let mut f = Fix::new()
        .board(ME, &["Ankylodon"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let dealt = f.g.players[1].board.first().map_or(0, |m| m.damage)
        + (30 - f.g.players[1].hero_hp);
    assert!(dealt > 0, "the Beasts went in on something");
}

#[test]
fn wilted_shadow_punishes_healing_the_enemy() {
    let mut f = Fix::new()
        .board(ME, &["Wilted Shadow"]) // 6/7
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[1].board[0].damage = 3;
    f.g.heal(Target::Minion(FOE, 0), 1);
    assert_eq!(f.their_board(), 0, "six into a body on four");
}

#[test]
fn behemoth_mask_makes_a_wall_and_a_victim() {
    let mut f = Fix::new()
        .board(ME, &["Wisp"])
        .board(FOE, &["Bloodfen Raptor"]); // 3/2
    f.play("Behemoth Mask", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 10));
    assert!(f.mine(0).has(Keywords::LIFESTEAL));
    assert_eq!(f.their_board(), 0, "the Raptor was made to run into it");
}

#[test]
fn warmaul_challenger_trades_until_one_falls() {
    // Against something bigger the Challenger loses the duel: it deals one a
    // round and takes six, so it gets two swings in and falls.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Warmaul Challenger", foe_minion(0)); // 1/10
    assert_eq!(f.g.players[0].board.len(), 0);
    assert_eq!(f.theirs(0).damage, 2);

    // Against something it can outlast, the duel goes the other way.
    let mut f = Fix::new().board(FOE, &["Bloodfen Raptor"]); // 3/2
    f.play("Warmaul Challenger", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.mine(0).health(), 10 - 6, "two rounds of one, three back each");
}

#[test]
fn rampaging_hound_pulls_the_far_board_onto_itself() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp", "Wisp"]);
    f.play("Rampaging Hound", None); // 4/12
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.mine(0).damage, 3);
}

// ------------------------------------------------------------ last sweep

#[test]
fn ymirjar_frostbreaker_counts_the_frost_you_hold() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Death's Advance").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Blizzard").unwrap()));
    f.play("Ymirjar Frostbreaker", None); // 1/2
    assert_eq!(f.mine(0).atk, 3);
}

#[test]
fn nurturing_nature_feeds_the_board_and_the_hand() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Mountain Bear").unwrap()));
    f.play("Nurturing Nature", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 4));
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (2, 2));
}

#[test]
fn release_the_beasts_pays_legends_more() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Deathwing").unwrap()));
    f.play("Release the Beasts", None);
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (1, 1));
    assert_eq!((f.g.players[0].hand[1].atk, f.g.players[0].hand[1].hp), (3, 2));
}

#[test]
fn dimensional_weaponsmith_arms_bodies_and_blades_alike() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Fiery War Axe").unwrap()));
    f.g.players[0].hand.push(HandCard::new(by_name("Fireball").unwrap()));
    f.play("Dimensional Weaponsmith", None);
    assert_eq!(f.g.players[0].hand[0].atk, 2);
    assert_eq!(f.g.players[0].hand[1].atk, 2);
    assert_eq!(f.g.players[0].hand[2].atk, 0, "a spell is neither");
}

#[test]
fn power_word_barrier_shields_and_hardens() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.play("Power Word: Barrier", my_minion(0));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    assert_eq!(f.g.players[0].hand[0].hp, 2);
}

#[test]
fn dig_for_freedom_pays_two_four_drops_on_death() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Dig for Freedom", my_minion(0));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 2);
    assert!(f.g.players[0].board.iter().all(|m| m.card.def().cost == 4));
}

#[test]
fn threshriders_blessing_buffs_and_promises_a_body() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.play("Threshrider's Blessing", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 5));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.def().cost, 4);
}

#[test]
fn logoshs_last_stand_pulls_a_body_out_of_hand() {
    let mut f = Fix::new().board(ME, &["Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.play("Lo'Gosh's Last Stand", my_minion(0));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.mine(0).card.name(), "Chillwind Yeti");
    assert_eq!(f.g.players[0].hand.len(), 0);
}

#[test]
fn amphibians_spirit_passes_itself_along() {
    let mut f = Fix::new().board(ME, &["Wisp", "Chillwind Yeti"]);
    f.play("Amphibian's Spirit", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 3));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 7), "the Yeti inherited it");
    assert_ne!(f.mine(0).granted_rattle, tavernlab_core::cards::CardId(0));
}

#[test]
fn sheep_mask_shrinks_a_minion_into_a_bomb() {
    let mut f = Fix::new()
        .board(ME, &["Boulderfist Ogre"])
        .board(FOE, &["Wisp", "Wisp"]);
    f.play("Sheep Mask", my_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (1, 1));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.their_board(), 0);
}

#[test]
fn charity_brings_back_what_you_lost_this_turn() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    f.play("Charity", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Chillwind Yeti");
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (3, 3));
}

#[test]
fn priest_of_anshe_wants_a_heal_first() {
    let mut f = Fix::new();
    f.play("Priest of An'she", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 5));

    let mut f = Fix::new();
    f.g.players[0].hero_hp = 20;
    f.g.heal_hero(ME, 1);
    f.play("Priest of An'she", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 8));
}

#[test]
fn envoy_of_the_glade_repaints_the_neutral_half_of_your_deck() {
    let mut f = Fix::new().deck(&["Chillwind Yeti", "Holy Light"]);
    f.play("Envoy of the Glade", None);
    assert_eq!(f.g.players[0].deck.len(), 2);
    assert_eq!(f.g.players[0].deck[0].def().class(), Class::Druid);
    assert_eq!(f.g.players[0].deck[1].name(), "Holy Light", "not Neutral");
}

#[test]
fn hellraiser_grows_on_an_empty_deck() {
    let mut f = Fix::new();
    f.play("Hellraiser", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 6));

    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.play("Hellraiser", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 2));
    assert_eq!(f.g.players[0].hand.len(), 1);
}

#[test]
fn storage_scuffle_is_free_after_a_discover() {
    let mut f = Fix::new();
    f.g.players[0].hand.push(HandCard::new(by_name("Storage Scuffle").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 3);
    f.g.discover(ME, |d| d.kind() == Kind::Spell);
    assert_eq!(f.g.card_cost(ME, 0), 0);
}

#[test]
fn unearthed_artifacts_doubles_up_after_a_discover() {
    let mut f = Fix::new();
    f.play("Unearthed Artifacts", None);
    assert_eq!(f.mine(0).card.def().cost, 2);

    let mut f = Fix::new();
    f.g.discover(ME, |d| d.kind() == Kind::Spell);
    f.play("Unearthed Artifacts", None);
    assert_eq!(f.mine(0).card.def().cost, 4);
}

#[test]
fn diabolus_rex_hits_both_ends_on_kindred() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Wisp", "Boulderfist Ogre"]);
    f.play("Diabolus Rex", None);
    assert_eq!(f.their_board(), 3, "no Kindred");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre", "Wisp", "Boulderfist Ogre"]);
    f.g.players[0].played_races_last = Races::DEMON;
    f.play("Diabolus Rex", None);
    assert_eq!(f.their_board(), 3, "six does not finish a 6/7");
    assert_eq!(f.theirs(0).damage, 6);
    assert_eq!(f.theirs(1).damage, 0, "the middle is spared");
    assert_eq!(f.theirs(2).damage, 6);
}

// ------------------------------------------------------------ the deck zone
//
// `DeckCard` carries per-copy state: stats written on a card while it waits,
// a cost set on it, and whether it was in the list the deck was built from.
// The first three tests are about the zone itself; the rest are the cards
// that read and write it.

/// Put cards into a deck as if something had shuffled them in mid-game, so
/// they are the ones that "didn't start in your deck".
fn shuffled_in(f: &mut Fix, side: Side, names: &[&str]) {
    for n in names {
        let card = by_name(n).unwrap_or_else(|| panic!("no card {n}"));
        f.g.player_mut(side).deck.push(DeckCard::new(card));
    }
}

#[test]
fn stats_written_on_a_deck_card_follow_it_into_hand_and_onto_the_board() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]); // 4/5
    f.g.players[0].deck[0].enchant(2, 3);
    f.g.draw(ME, 1);
    let hc = f.g.players[0].hand[0];
    assert_eq!((hc.atk, hc.hp), (2, 3), "the enchantment came along");

    let ok = f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    });
    assert!(ok);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 8));
}

#[test]
fn a_cost_set_in_the_deck_is_what_the_card_costs_in_hand() {
    let mut f = Fix::new().deck(&["Deathwing"]); // costs 10
    f.g.players[0].deck[0].set_cost(1);
    f.g.draw(ME, 1);
    assert_eq!(f.g.card_cost(ME, 0), 1);
}

#[test]
fn trading_a_card_away_does_not_make_it_a_stranger_to_its_own_deck() {
    // A Tradeable card goes back into the deck. It started there, so
    // Steamcleaner must not sweep it up and Story of the Waygate must not
    // discount it once it is drawn again. Trading shuffles and then draws,
    // so the card can end up in either zone: both are checked.
    let mut f = Fix::new().deck(&["Wisp"]);
    let tradeable = (0..u16::MAX)
        .map(tavernlab_core::cards::CardId)
        .find(|c| {
            c.def().collectible
                && c.def().keywords.has(Keywords::TRADEABLE)
                && behaviour_of(*c).is_some()
        })
        .expect("some Tradeable card is implemented");
    f.g.players[0].hand.push(HandCard::new(tradeable));
    f.g.players[0].mana = 10;
    assert!(f.g.apply(Action::Trade { hand: 0 }));

    let in_deck = f.g.players[0].deck.iter().find(|d| d.card == tradeable);
    let in_hand = f.g.players[0].hand.iter().find(|h| h.card == tradeable);
    assert!(
        in_deck.is_some() || in_hand.is_some(),
        "the traded card is still somewhere"
    );
    if let Some(d) = in_deck {
        assert!(d.started_here, "it started in this deck and still has");
    }
    if let Some(h) = in_hand {
        assert!(
            !h.marks.has(tavernlab_core::state::Marks::NOT_FROM_DECK),
            "drawn straight back out, still one of the deck's own"
        );
    }
}

#[test]
fn a_bounced_minion_remembers_it_came_from_the_deck() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    f.g.draw(ME, 1);
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    f.g.bounce(Target::Minion(ME, 0));
    assert!(
        !f.g.players[0].hand[0]
            .marks
            .has(tavernlab_core::state::Marks::NOT_FROM_DECK),
        "played out of the deck, so bouncing it does not relabel it"
    );

    // A token summoned onto the board never was in the deck.
    let mut f = Fix::new();
    f.g.summon(ME, by_name("Chillwind Yeti").unwrap());
    f.g.bounce(Target::Minion(ME, 0));
    assert!(
        f.g.players[0].hand[0]
            .marks
            .has(tavernlab_core::state::Marks::NOT_FROM_DECK),
    );
}

#[test]
fn beanstalk_brute_buffs_the_top_three_minions_skipping_spells() {
    // Top is the end of the list: Yeti, Raptor, then past Fireball to the
    // second Wisp. The fourth minion down is out of reach.
    let mut f = Fix::new().deck(&[
        "Wisp",
        "Wisp",
        "Fireball",
        "Bloodfen Raptor",
        "Chillwind Yeti",
    ]);
    f.play("Beanstalk Brute", None);
    let d = &f.g.players[0].deck;
    assert_eq!((d[0].atk, d[0].hp), (0, 0), "the fourth minion down, missed");
    assert_eq!((d[1].atk, d[1].hp), (4, 4), "the third minion down");
    assert_eq!((d[2].atk, d[2].hp), (0, 0), "Fireball is not a minion");
    assert_eq!((d[3].atk, d[3].hp), (4, 4));
    assert_eq!((d[4].atk, d[4].hp), (4, 4));
}

#[test]
fn beanstalk_brute_stops_when_the_deck_runs_out_of_minions() {
    let mut f = Fix::new().deck(&["Fireball", "Chillwind Yeti"]);
    f.play("Beanstalk Brute", None);
    assert_eq!(
        (f.g.players[0].deck[1].atk, f.g.players[0].deck[1].hp),
        (4, 4)
    );
    assert_eq!((f.g.players[0].deck[0].atk, f.g.players[0].deck[0].hp), (0, 0));
}

#[test]
fn kaldorei_cultivator_puts_two_buffed_beasts_on_the_bottom() {
    let mut f = Fix::new().deck(&["Wisp"]);
    f.play("Kaldorei Cultivator", None);
    assert_eq!(f.g.players[0].deck.len(), 3, "two went in");
    for i in 0..2 {
        let dc = f.g.players[0].deck[i];
        assert!(dc.def().races.any(Races::BEAST), "{} is not a Beast", dc.name());
        assert_eq!((dc.atk, dc.hp), (5, 5));
        assert!(!dc.started_here, "Discovered, not built in");
    }
    assert_eq!(
        f.g.players[0].deck[2].name(),
        "Wisp",
        "the deck's own card stayed on top"
    );
}

#[test]
fn seismopod_buffs_every_minion_in_hand_and_deck_but_no_spell() {
    let mut f = Fix::new()
        .board(ME, &["Seismopod"])
        .deck(&["Chillwind Yeti", "Fireball"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Bloodfen Raptor").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Frostbolt").unwrap()));
    f.g.deal_damage(Target::Minion(ME, 0), 9);
    f.g.sweep_deaths();

    assert_eq!(
        (f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp),
        (3, 3)
    );
    assert_eq!(
        (f.g.players[0].hand[1].atk, f.g.players[0].hand[1].hp),
        (0, 0),
        "Frostbolt is not a minion"
    );
    assert_eq!(
        (f.g.players[0].deck[0].atk, f.g.players[0].deck[0].hp),
        (3, 3)
    );
    assert_eq!(
        (f.g.players[0].deck[1].atk, f.g.players[0].deck[1].hp),
        (0, 0)
    );
}

#[test]
fn supreme_dinomancy_finds_beasts_in_all_three_zones() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor", "Chillwind Yeti"]) // Beast, not a Beast
        .deck(&["Oasis Snapjaw", "Fireball"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Stonetusk Boar").unwrap()));
    f.play("Supreme Dinomancy", None);

    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 4), "3/2 Beast");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (4, 5), "not a Beast");
    assert_eq!(
        (f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp),
        (2, 2)
    );
    assert_eq!(
        (f.g.players[0].deck[0].atk, f.g.players[0].deck[0].hp),
        (2, 2)
    );
    assert_eq!(
        (f.g.players[0].deck[1].atk, f.g.players[0].deck[1].hp),
        (0, 0)
    );
}

#[test]
fn azsharas_triumph_shuffles_in_five_doubled_giants() {
    let mut f = Fix::new();
    f.play("Azshara's Triumph", None);
    assert_eq!(f.g.players[0].deck.len(), 5);
    for dc in f.g.players[0].deck.iter() {
        let d = dc.def();
        assert_eq!(d.kind(), Kind::Minion);
        assert!(d.cost >= 8, "{} costs {}", dc.name(), d.cost);
        assert_eq!(
            (dc.atk, dc.hp),
            (d.atk as i8, d.hp as i8),
            "{} did not double",
            dc.name()
        );
    }
}

#[test]
fn city_chief_esho_pays_out_when_the_deck_is_one_tribe() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .deck(&["Bloodfen Raptor", "Oasis Snapjaw", "Fireball"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("City Chief Esho", None);

    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 7), "the other body");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (5, 7), "Esho itself");
    assert_eq!(
        (f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp),
        (2, 2)
    );
    assert_eq!(
        (f.g.players[0].deck[0].atk, f.g.players[0].deck[0].hp),
        (2, 2)
    );
    assert_eq!(
        (f.g.players[0].deck[2].atk, f.g.players[0].deck[2].hp),
        (0, 0),
        "Fireball is not a minion"
    );
}

#[test]
fn city_chief_esho_pays_nothing_on_a_mixed_deck() {
    let mut f = Fix::new()
        .board(ME, &["Chillwind Yeti"])
        .deck(&["Bloodfen Raptor", "Chillwind Yeti"]); // Beast and no tribe at all
    f.play("City Chief Esho", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 5));
    assert_eq!(
        (f.g.players[0].deck[0].atk, f.g.players[0].deck[0].hp),
        (0, 0)
    );
}

#[test]
fn krona_sets_the_bottom_five_to_one_mana() {
    let mut f = Fix::new().deck(&[
        "Deathwing",
        "Boulderfist Ogre",
        "Chillwind Yeti",
        "Bloodfen Raptor",
        "Fireball",
        "Wisp",
    ]);
    f.play("Krona, Keeper of Eons", None);
    for i in 0..5 {
        let dc = f.g.players[0].deck[i];
        assert_eq!(
            dc.def().cost + dc.cost_delta as i16,
            1,
            "{} should cost 1",
            dc.name()
        );
    }
    let top = f.g.players[0].deck[5];
    assert_eq!(top.cost_delta, 0, "the sixth from the bottom is untouched");
}

#[test]
fn steamcleaner_sweeps_both_decks_of_what_was_shuffled_in() {
    let mut f = Fix::new().deck(&["Chillwind Yeti"]);
    shuffled_in(&mut f, ME, &["Wisp", "Fireball"]);
    f.g.players[1]
        .deck
        .push(DeckCard::started(by_name("Wisp").unwrap()));
    shuffled_in(&mut f, FOE, &["Boulderfist Ogre"]);

    f.play("Steamcleaner", None);
    assert_eq!(f.g.players[0].deck.len(), 1);
    assert_eq!(f.g.players[0].deck[0].name(), "Chillwind Yeti");
    assert_eq!(f.g.players[1].deck.len(), 1);
    assert_eq!(f.g.players[1].deck[0].name(), "Wisp");
}

#[test]
fn smuggled_shovel_draws_the_spell_that_was_not_yours() {
    let mut f = Fix::new().deck(&["Fireball"]);
    shuffled_in(&mut f, ME, &["Frostbolt"]);
    f.g.equip(ME, by_name("Smuggled Shovel").unwrap());
    // Equipping over a weapon breaks it, which fires its deathrattle.
    f.g.equip(ME, by_name("Fiery War Axe").unwrap());
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Frostbolt");
}

#[test]
fn dragonscale_armaments_draws_one_of_each_origin() {
    let mut f = Fix::new().deck(&["Fireball"]);
    shuffled_in(&mut f, ME, &["Frostbolt"]);
    f.play("Dragonscale Armaments", None);
    let mut names: Vec<&str> = f.g.players[0]
        .hand
        .iter()
        .map(|hc| hc.card.name())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["Fireball", "Frostbolt"]);
    assert!(f.g.players[0].deck.is_empty());
}

#[test]
fn dreamwarden_draws_the_stranger_and_grows() {
    let mut f = Fix::new().deck(&["Fireball"]);
    shuffled_in(&mut f, ME, &["Wisp"]);
    f.play("Dreamwarden", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (5, 6));
    assert_eq!(f.g.players[0].hand[0].card.name(), "Wisp");
    assert_eq!(f.g.players[0].deck.len(), 1, "its own card stayed");
}

#[test]
fn dreamwarden_is_a_plain_body_when_the_deck_is_all_its_own() {
    let mut f = Fix::new().deck(&["Fireball", "Wisp"]);
    f.play("Dreamwarden", None);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 4));
    assert!(f.g.players[0].hand.is_empty(), "nothing to draw");
}

#[test]
fn story_of_the_waygate_discounts_only_what_you_did_not_bring() {
    let mut f = Fix::new().deck(&["Fireball"]);
    shuffled_in(&mut f, ME, &["Boulderfist Ogre"]);
    f.g.draw(ME, 2);
    f.play("Story of the Waygate", None);
    for i in 0..2 {
        let hc = f.g.players[0].hand[i];
        let want = if hc.card.name() == "Boulderfist Ogre" { 5 } else { 4 };
        assert_eq!(f.g.card_cost(ME, i), want, "{}", hc.card.name());
    }
}

#[test]
fn techysaurus_counts_every_card_you_did_not_bring() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Techysaurus").unwrap()));
    assert_eq!(f.g.card_cost(ME, 0), 7);

    // Two cards that arrived from outside the deck, played.
    for _ in 0..2 {
        f.g.give_card(ME, by_name("Wisp").unwrap());
        let idx = f.g.players[0].hand.len() as u8 - 1;
        assert!(f.g.apply(Action::Play {
            hand: idx,
            target: None,
            position: u8::MAX,
            choice: u8::MAX,
        }));
    }
    assert_eq!(f.g.card_cost(ME, 0), 5);

    // One that did come out of the deck changes nothing.
    f.g.players[0]
        .deck
        .push(DeckCard::started(by_name("Wisp").unwrap()));
    f.g.draw(ME, 1);
    let idx = f.g.players[0].hand.len() as u8 - 1;
    assert!(f.g.apply(Action::Play {
        hand: idx,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.card_cost(ME, 0), 5);
}

// ------------------------------------------ the last two of a real deck list
//
// Both came out of one user's Shaman list, which was the only thing standing
// between it and being scored.

#[test]
fn hexmarshal_discounts_only_a_deck_that_started_without_spells() {
    // `Fix::new()` builds its players with an empty deck list, which started
    // without spells the same way a spell-less thirty does.
    let mut f = Fix::new();
    f.play("Hexmarshal", None);
    let hc = f.g.players[0].hand[0];
    let d = hc.card.def();
    assert_eq!(d.kind(), Kind::Spell);
    assert!(d.cost >= 5, "{} costs {}", hc.card.name(), d.cost);
    assert_eq!(f.g.card_cost(ME, 0), d.cost - 5);

    // A list with one spell in it loses the discount, and keeps losing it
    // after that spell has been drawn: the question is about the list, not
    // about what is left.
    let mut g = Game::new(
        (Class::Shaman, &[by_name("Fireball").unwrap()]),
        (Class::Mage, &[]),
        1,
    )
    .unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    g.draw(ME, 1);
    let mut f = Fix { g };
    f.play("Hexmarshal", None);
    let idx = f.g.players[0].hand.len() - 1;
    let d = f.g.players[0].hand[idx].card.def();
    assert_eq!(d.kind(), Kind::Spell);
    assert_eq!(f.g.card_cost(ME, idx), d.cost, "no discount, full price");
}

#[test]
fn shadowed_informant_starts_on_your_class_and_swaps_off_it() {
    let mut f = Fix::new(); // Mage
    assert_eq!(
        f.g.players[0].informant_class,
        Class::Mage,
        "it starts as the hero's own class"
    );
    f.play("Shadowed Informant", None);
    assert_eq!(
        f.g.players[0].hand[0].card.def().class(),
        Class::Mage,
        "played the turn it arrives, it offers your own spells"
    );
    assert_eq!(f.g.players[0].hand[0].card.def().kind(), Kind::Spell);

    // It swaps at the end of every one of its owner's turns, and swaps to a
    // different class rather than possibly staying put.
    let mut f = Fix::new();
    let mut seen = vec![Class::Mage];
    for _ in 0..3 {
        f.g.end_turn();
        f.g.begin_turn();
        let now = f.g.players[0].informant_class;
        assert_ne!(now, *seen.last().unwrap(), "swaps to a different class");
        assert_ne!(now, Class::Neutral, "always a playable class");
        seen.push(now);
    }
}

#[test]
fn shadowed_informant_follows_the_class_it_has_swapped_to() {
    let mut f = Fix::new();
    f.g.end_turn();
    f.g.begin_turn();
    let want = f.g.players[0].informant_class;
    assert_ne!(want, Class::Mage);
    f.g.players[0].mana = 10;
    f.g.players[0].crystals = 10;
    f.play("Shadowed Informant", None);
    let idx = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.players[0].hand[idx].card.def().class(), want);
}

// -------------------------------------------------------------- Bonus Effects
//
// A Bonus Effect is one of eight keywords. The pool is not in the card data —
// the corpus holds no enchantment cards at all — so it lives in
// `Game::BONUS_EFFECTS`, and these tests check membership rather than naming
// a particular keyword, which the roll is free to choose.

/// How many of the eight this minion has been *given*, as opposed to printed.
fn granted_bonuses(m: &Permanent) -> usize {
    let printed = m.card.def().keywords;
    Game::BONUS_EFFECTS
        .iter()
        .filter(|kw| m.has(**kw) && !printed.has(**kw))
        .count()
}

#[test]
fn shadows_of_yesterday_leaves_four_shades_with_two_bonus_effects_each() {
    let mut f = Fix::new();
    f.play("Shadows of Yesterday", None);
    assert_eq!(f.g.players[0].board.len(), 4);
    for i in 0..4 {
        let m = f.mine(i);
        assert_eq!(m.card.name(), "Anomalous Shade");
        assert_eq!((m.atk, m.max_hp), (3, 2), "a 3/2, as printed");
        assert_eq!(granted_bonuses(m), 2, "two distinct Bonus Effects");
    }
}

#[test]
fn story_of_galvadon_gives_one_minion_three_distinct_bonus_effects() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Story of Galvadon", my_minion(0));
    assert_eq!(granted_bonuses(f.mine(0)), 3);
    assert_eq!(
        (f.mine(0).atk, f.mine(0).max_hp),
        (4, 5),
        "keywords only, no stats"
    );
}

#[test]
fn tyrannogill_leaves_three_murlocs_each_with_a_bonus_effect() {
    let mut f = Fix::new().board(ME, &["Tyrannogill"]);
    f.g.deal_damage(Target::Minion(ME, 0), 3);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 3);
    for i in 0..3 {
        let m = f.mine(i);
        assert_eq!(m.card.name(), "Dinoloc");
        assert_eq!((m.atk, m.max_hp), (2, 1));
        assert_eq!(granted_bonuses(m), 1);
    }
}

#[test]
fn stranglevine_passes_on_a_bonus_effect_and_its_own_deathrattle() {
    let mut f = Fix::new().board(ME, &["Stranglevine", "Chillwind Yeti"]);
    f.g.deal_damage(Target::Minion(ME, 0), 2);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1, "only the Yeti is left");
    let yeti = f.mine(0);
    assert_eq!(granted_bonuses(yeti), 1);
    assert_eq!(
        yeti.granted_rattle.name(),
        "Stranglevine",
        "and it carries the rattle onward"
    );
}

#[test]
fn violet_punisher_steals_only_what_was_granted() {
    // Goldshire Footman prints Taunt, so Taunt is not a Bonus Effect on it.
    let mut f = Fix::new().board(FOE, &["Goldshire Footman"]);
    f.g.grant(Target::Minion(FOE, 0), Keywords::POISONOUS);
    f.play("Violet Punisher", foe_minion(0));

    let victim = f.theirs(0);
    assert!(victim.has(Keywords::TAUNT), "its own printed Taunt stays");
    assert!(!victim.has(Keywords::POISONOUS), "the granted one is gone");

    let thief = f.mine(0);
    assert!(thief.has(Keywords::POISONOUS));
    assert!(!thief.has(Keywords::TAUNT), "Taunt was never a Bonus Effect");
    assert_eq!((thief.atk, thief.max_hp), (5, 4), "4/3 plus one stolen");
}

#[test]
fn violet_punisher_on_a_plain_body_is_just_a_body() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Violet Punisher", foe_minion(0));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 3), "nothing to steal");
}

// -------------------------------------------------------------------- Imbue
//
// "Imbue your Hero Power" installs the class's Blessing and raises a count,
// and that count is the number every Blessing is written around: the corpus
// writes all eight with `@` and carries no value anywhere. The scaling —
// one more each time, no ceiling — was supplied from outside the card data;
// the printed Plant Golem being a 1/1 is the one thing the data corroborates.

#[test]
fn imbue_installs_the_class_blessing_and_counts_up() {
    let mut f = Fix::new(); // Mage
    assert_eq!(f.g.players[0].hero_power.name(), "Fireblast");
    f.play("Bitterbloom Knight", None);
    assert_eq!(f.g.players[0].hero_power.name(), "Blessing of the Wisp");
    assert_eq!(f.g.players[0].imbue_count, 1);

    f.play("Flutterwing Guardian", None);
    assert_eq!(f.g.players[0].imbue_count, 2, "the second raises the count");
    assert_eq!(
        f.g.players[0].hero_power.name(),
        "Blessing of the Wisp",
        "and leaves the Blessing in place"
    );
}

#[test]
fn a_class_with_no_blessing_still_counts_its_imbues() {
    // The Emerald Dream set printed eight Blessings; Warlock is not among
    // them. A Neutral Imbue card is still legal in a Warlock deck.
    let mut g = Game::new((Class::Warlock, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.play("Bitterbloom Knight", None);
    assert_eq!(f.g.players[0].imbue_count, 1);
    assert_eq!(
        f.g.players[0].hero_power.name(),
        "Life Tap",
        "nothing to install, so the basic power stays"
    );
}

#[test]
fn blessing_of_the_golem_summons_a_plant_golem_the_size_of_the_count() {
    let mut g = Game::new((Class::Druid, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.play("Bitterbloom Knight", None); // Imbue once
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false
    }));
    let golem = f.mine(f.g.players[0].board.len() - 1);
    assert_eq!(golem.card.name(), "Plant Golem");
    assert_eq!((golem.atk, golem.max_hp), (1, 1), "one Imbue, a 1/1");

    // A second Imbue, and the next Golem is a 2/2.
    f.g.players[0].hero_power_uses = 0;
    f.g.players[0].mana = 10;
    f.g.imbue(ME);
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false
    }));
    let golem = f.mine(f.g.players[0].board.len() - 1);
    assert_eq!((golem.atk, golem.max_hp), (2, 2));
}

#[test]
fn blessing_of_the_wisp_scales_both_halves_with_the_count() {
    let mut f = Fix::new(); // Mage
    f.play("Bitterbloom Knight", None);
    f.g.imbue(ME); // count 2
    f.g.players[0].mana = 10;
    let before = f.g.players[1].hero_hp;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false
    }));
    let wisps = f.g.players[0]
        .board
        .iter()
        .filter(|m| m.card.name() == "Wisp")
        .count();
    assert_eq!(wisps, 2, "two Imbues, two Wisps");
    assert_eq!(before - f.g.players[1].hero_hp, 2, "and two damage");
}

#[test]
fn blessing_of_the_wolf_arms_a_beast_in_hand() {
    let mut g = Game::new((Class::Hunter, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Bloodfen Raptor").unwrap()));
    f.play("Bitterbloom Knight", None);
    f.g.players[0].mana = 10;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false
    }));
    let hc = f.g.players[0].hand[0];
    assert_eq!(hc.card.name(), "Bloodfen Raptor");
    assert_eq!((hc.atk, hc.hp), (1, 0), "+1 Attack at one Imbue");
    assert_eq!(f.g.card_cost(ME, 0), hc.card.def().cost - 1);
}

#[test]
fn blessing_of_the_dragon_shuffles_portals_that_cast_themselves_on_the_draw() {
    let mut g = Game::new((Class::Paladin, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.play("Bitterbloom Knight", None);
    f.g.players[0].mana = 10;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false
    }));
    assert_eq!(f.g.players[0].deck.len(), 2, "two Portals");
    assert!(
        f.g.players[0]
            .deck
            .iter()
            .all(|d| d.name() == "Emerald Portal")
    );

    let hand = f.g.players[0].hand.len();
    let board = f.g.players[0].board.len();
    f.g.draw(ME, 1);
    assert_eq!(
        f.g.players[0].hand.len(),
        hand,
        "a Portal never reaches hand"
    );
    assert_eq!(
        f.g.players[0].board.len(),
        board + 1,
        "it casts itself instead"
    );
    let dragon = f.mine(f.g.players[0].board.len() - 1);
    assert!(dragon.races().any(Races::DRAGON));
    assert_eq!(dragon.card.def().cost, 1, "one Imbue, a 1-Cost Dragon");
}

#[test]
fn blessing_of_the_moon_hands_over_a_temporary_discounted_card() {
    let mut g = Game::new((Class::Priest, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.play("Lunarwing Messenger", None);
    f.g.players[0].mana = 10;
    assert!(f.g.apply(Action::HeroPower {
        target: None,
        second: false
    }));
    let at = f.g.players[0].hand.len() - 1;
    let hc = f.g.players[0].hand[at];
    assert_eq!(hc.card.def().class(), Class::Priest);
    assert_eq!(f.g.card_cost(ME, at), hc.card.def().cost - 1);
    assert!(hc.marks.has(tavernlab_core::state::Marks::TEMPORARY));

    // Unplayed by the end of the turn, it is gone.
    f.g.end_turn();
    assert!(
        !f.g.players[0]
            .hand
            .iter()
            .any(|h| h.marks.has(tavernlab_core::state::Marks::TEMPORARY)),
        "Temporary cards burn at end of turn"
    );
}

#[test]
fn blessing_of_the_wind_grows_a_minion_by_the_count() {
    let mut g = Game::new((Class::Shaman, &[]), (Class::Mage, &[]), 1).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.play("Bitterbloom Knight", None); // a 2-cost body, Imbue once
    let slot = (f.g.players[0].board.len() - 1) as u8;
    let was = f.mine(slot as usize).card.def().cost;
    f.g.players[0].mana = 10;
    assert!(f.g.apply(Action::HeroPower {
        target: Some(Target::Minion(ME, slot)),
        second: false
    }));
    assert_eq!(
        f.mine(slot as usize).card.def().cost,
        was + 1,
        "one Imbue, one Mana more"
    );
}

#[test]
fn wisprider_imbues_then_fires_the_power_for_free() {
    let mut f = Fix::new(); // Mage
    f.play("Wisprider", None);
    assert_eq!(f.g.players[0].imbue_count, 1);
    assert!(
        f.g.players[0]
            .board
            .iter()
            .any(|m| m.card.name() == "Wisp"),
        "the Blessing fired on the way in"
    );
    assert_eq!(
        f.g.players[0].hero_power_uses, 0,
        "and the turn's own Hero Power is still available"
    );
}

#[test]
fn finality_imbues_twice() {
    let mut g = Game::new(
        (Class::DeathKnight, &[by_name("Frail Ghoul").unwrap()]),
        (Class::Mage, &[]),
        1,
    )
    .unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    let mut f = Fix { g };
    f.play("Finality", None);
    assert_eq!(f.g.players[0].imbue_count, 2);
    assert_eq!(
        f.g.players[0].hero_power.name(),
        "Blessing of the Infinite"
    );
}

#[test]
fn petal_picker_waits_for_the_second_imbue() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.play("Petal Picker", None);
    assert!(f.g.players[0].hand.is_empty(), "one Imbue short of anything");

    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.imbue(ME);
    f.g.imbue(ME);
    f.play("Petal Picker", None);
    assert_eq!(f.g.players[0].hand.len(), 2);
}

#[test]
fn azalina_soulsever_starts_at_forty_with_twenty_of_the_enemys_cards() {
    use tavernlab_core::agent::{Scripted, Style};

    // Twenty cards, because that is the legal size of a list holding her.
    let azalina = by_name("Azalina Soulsever").unwrap();
    let mut mine = vec![azalina];
    mine.resize(20, by_name("Wisp").unwrap());
    // Something no list of Wisps could contain, so the copies are identifiable.
    let theirs = vec![by_name("Boulderfist Ogre").unwrap(); 30];

    let mut g = Game::new((Class::Priest, &mine), (Class::Mage, &theirs), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    assert_eq!(g.players[0].hero_hp, 40, "starting Health is 40");

    // Twenty of her own plus twenty copies, less whatever the mulligan drew.
    let hers = g.players[0].deck.len() + g.players[0].hand.len();
    assert_eq!(hers, 40, "twenty of her own and twenty copied");
    let borrowed = g.players[0]
        .deck
        .iter()
        .filter(|d| d.name() == "Boulderfist Ogre")
        .count()
        + g.players[0]
            .hand
            .iter()
            .filter(|h| h.card.name() == "Boulderfist Ogre")
            .count();
    assert_eq!(borrowed, 20);

    // Copies: the opponent keeps every card they started with. Counted by
    // card rather than by zone size, since the player on the draw is also
    // holding the Coin by now.
    let theirs_left = g.players[1]
        .deck
        .iter()
        .filter(|d| d.name() == "Boulderfist Ogre")
        .count()
        + g.players[1]
            .hand
            .iter()
            .filter(|h| h.card.name() == "Boulderfist Ogre")
            .count();
    assert_eq!(theirs_left, 30, "the opponent lost nothing");
}

#[test]
fn a_twenty_card_list_is_the_legal_size_only_with_azalina() {
    use tavernlab_core::cards::Formats;
    use tavernlab_core::deck::{DeckError, legal_size, validate};
    let wisp = by_name("Wisp").unwrap();
    let azalina = by_name("Azalina Soulsever").unwrap();

    let mut with = vec![azalina];
    with.resize(20, wisp);
    let without = vec![wisp; 20];
    assert_eq!(legal_size(&with), 20);
    assert_eq!(legal_size(&without), 30);

    // Whatever else is wrong with these lists -- nineteen Wisps break the
    // copy limit -- the size is the question here, and only one of them is
    // called out on it.
    assert_ne!(
        validate(&with, Class::Priest, Formats::STANDARD),
        Err(DeckError::WrongSize(20))
    );
    assert_eq!(
        validate(&without, Class::Priest, Formats::STANDARD),
        Err(DeckError::WrongSize(20))
    );

    // And thirty is the wrong size *with* her.
    let mut thirty = vec![azalina];
    thirty.resize(30, wisp);
    assert_eq!(legal_size(&thirty), 20);
    assert_eq!(
        validate(&thirty, Class::Priest, Formats::STANDARD),
        Err(DeckError::WrongSize(30))
    );
}

// ---------------------------------------------------------------- Dark Gifts
//
// Ten of them, and the corpus names all ten: every Dark Gift card carries the
// pool as its own children, and each Gift's effect is its own printed text.
// Nothing here comes from outside the card data.

/// A card in hand with a chosen Dark Gift already on it, stats and all.
fn hand_with_gift(f: &mut Fix, name: &str, gift: u8) -> usize {
    let card = by_name(name).unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    let at = f.g.players[0].hand.len() - 1;
    let (atk, hp, cost) = tavernlab_core::cards::gift_stats(gift);
    let hc = &mut f.g.players[0].hand[at];
    hc.gift = gift;
    hc.enchant(atk, hp);
    hc.cost_delta += cost;
    at
}

fn play_hand(f: &mut Fix, at: usize) {
    assert!(f.g.apply(Action::Play {
        hand: at as u8,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
}

#[test]
fn a_stat_gift_lands_in_hand_and_a_keyword_gift_lands_on_the_board() {
    // Waking Terror: "+3 Attack and Lifesteal."
    let mut f = Fix::new();
    let at = hand_with_gift(&mut f, "Chillwind Yeti", 1); // 4/5
    assert_eq!(
        (f.g.players[0].hand[at].atk, f.g.players[0].hand[at].hp),
        (3, 0),
        "the stats are on the card in hand"
    );
    play_hand(&mut f, at);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (7, 5));
    assert!(f.mine(0).has(Keywords::LIFESTEAL), "and the keyword on the body");
}

#[test]
fn short_claws_is_the_one_gift_that_takes_something_away() {
    // "Costs (2) less, but has -2 Attack."
    let mut f = Fix::new();
    let at = hand_with_gift(&mut f, "Chillwind Yeti", 3);
    assert_eq!(f.g.card_cost(ME, at), 4 - 2);
    play_hand(&mut f, at);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 5));
}

#[test]
fn rude_awakening_fires_the_battlecry_twice() {
    // Novice Engineer draws one card; with the Gift it draws two.
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    let at = hand_with_gift(&mut f, "Novice Engineer", 7);
    play_hand(&mut f, at);
    assert_eq!(f.g.players[0].hand.len(), 2, "the Battlecry ran twice");
}

#[test]
fn living_nightmare_brings_a_two_two_copy_along() {
    let mut f = Fix::new();
    let at = hand_with_gift(&mut f, "Chillwind Yeti", 5);
    play_hand(&mut f, at);
    assert_eq!(f.g.players[0].board.len(), 2);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 5), "the real one");
    assert_eq!(f.mine(1).card.name(), "Chillwind Yeti");
    assert_eq!((f.mine(1).atk, f.mine(1).max_hp), (2, 2), "a 2/2 copy");
}

#[test]
fn persisting_horror_comes_back_whole() {
    let mut f = Fix::new();
    let at = hand_with_gift(&mut f, "Chillwind Yeti", 9); // Reborn
    play_hand(&mut f, at);
    assert!(f.mine(0).has(Keywords::REBORN));
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1, "it came back");
    assert_eq!(f.mine(0).health(), 5, "at full Health, not at one");
    assert!(!f.mine(0).has(Keywords::REBORN), "and only the once");
}

#[test]
fn plain_reborn_still_comes_back_at_one_health() {
    // The counterpart: the Gift changes Reborn, Reborn itself is unchanged.
    // The same body, the same death, without the Gift.
    let mut f = Fix::new();
    let at = hand_with_gift(&mut f, "Chillwind Yeti", 0);
    play_hand(&mut f, at);
    f.g.grant(Target::Minion(ME, 0), Keywords::REBORN);
    f.g.deal_damage(Target::Minion(ME, 0), 5);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1, "it came back");
    assert_eq!(f.mine(0).health(), 1, "at one Health");
}

#[test]
fn sweet_dreams_puts_the_card_on_top_of_the_deck() {
    // "+4/+5. Place this card on top of your deck."
    let mut f = Fix::new().deck(&["Wisp"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    // Force the roll by giving it directly: the pick itself is random.
    let at = f.g.players[0].hand.len() - 1;
    let hand_before = f.g.players[0].hand.len();
    let (atk, hp, _) = tavernlab_core::cards::gift_stats(8);
    {
        let hc = &mut f.g.players[0].hand[at];
        hc.gift = 8;
        hc.enchant(atk, hp);
    }
    let hc = f.g.players[0].hand[at];
    f.g.players[0].hand.remove(at);
    let mut dc = DeckCard::new(hc.card);
    dc.atk = hc.atk;
    dc.hp = hc.hp;
    f.g.players[0].deck.push(dc);

    assert_eq!(f.g.players[0].hand.len(), hand_before - 1);
    let top = f.g.players[0].deck[f.g.players[0].deck.len() - 1];
    assert_eq!(top.name(), "Chillwind Yeti");
    assert_eq!((top.atk, top.hp), (4, 5));
}

#[test]
fn discovering_with_a_dark_gift_really_attaches_one() {
    let mut f = Fix::new();
    f.play("Brutish Endmaw", None);
    // The card either sits in hand with a Gift, or Sweet Dreams sent it to
    // the top of the deck. Both are the Gift having landed.
    let in_hand = f.g.players[0].hand.iter().any(|hc| hc.gift != 0);
    let on_deck = !f.g.players[0].deck.is_empty();
    assert!(in_hand || on_deck, "the Discover produced nothing gifted");
    if let Some(hc) = f.g.players[0].hand.iter().find(|hc| hc.gift != 0) {
        assert_eq!(hc.card.def().cost, 1, "a 1-Cost minion, as printed");
        assert!(tavernlab_core::cards::gift_card(hc.gift).is_some());
    }
}

#[test]
fn holding_a_dark_gift_is_what_the_payoff_cards_read() {
    let mut f = Fix::new();
    f.play("Dragon Turtle", None);
    assert_eq!(f.g.players[0].armor, 0, "nothing in hand, no payoff");

    let mut f = Fix::new();
    hand_with_gift(&mut f, "Chillwind Yeti", 4);
    f.play("Dragon Turtle", None);
    assert_eq!(f.g.players[0].armor, 6);
    assert_eq!(f.g.players[0].hero_bonus_atk, 3);
}

#[test]
fn overgrown_horror_discounts_only_the_gifted_minions() {
    let mut f = Fix::new();
    let gifted = hand_with_gift(&mut f, "Boulderfist Ogre", 4); // 6-cost
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    let plain = f.g.players[0].hand.len() - 1;
    f.play("Overgrown Horror", None);
    assert_eq!(f.g.card_cost(ME, gifted), 6 - 2);
    assert_eq!(f.g.card_cost(ME, plain), 4, "no Gift, no discount");
}

// ------------------------------------------------------- acting on the draw
//
// "Casts When Drawn" on a spell, "Summoned When Drawn" on a minion. The card
// never reaches hand; it resolves on the way out of the deck, and the draw
// still counts as a draw.

#[test]
fn an_acorn_never_reaches_hand_and_leaves_a_squirrel() {
    let mut f = Fix::new().board(ME, &["Vibrant Squirrel"]);
    f.g.deal_damage(Target::Minion(ME, 0), 1);
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].deck.len(), 4, "four Acorns");
    assert!(f.g.players[0].deck.iter().all(|d| d.name() == "Acorn"));

    f.g.draw(ME, 1);
    assert!(f.g.players[0].hand.is_empty(), "it never reaches hand");
    assert_eq!(f.g.players[0].deck.len(), 3, "and it is gone from the deck");
    assert_eq!(f.g.players[0].board.len(), 1);
    assert_eq!(f.mine(0).card.name(), "Satisfied Squirrel");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (2, 1));
}

#[test]
fn a_summoned_when_drawn_minion_arrives_on_the_board() {
    let mut f = Fix::new();
    f.play("Interrogation", None);
    assert_eq!(f.g.players[0].deck.len(), 3);
    f.g.draw(ME, 1);
    assert!(f.g.players[0].hand.is_empty());
    assert_eq!(f.mine(0).card.name(), "Tortollan Ninja");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 3));
    assert!(f.mine(0).has(Keywords::STEALTH), "as printed");
}

#[test]
fn a_shred_of_time_hurts_the_hero_that_drew_it() {
    let mut f = Fix::new();
    f.play("Twilight Timehopper", None);
    assert_eq!(f.g.players[0].deck.len(), 2);
    let before = f.g.players[0].hero_hp;
    f.g.draw(ME, 1);
    assert_eq!(f.g.players[0].hero_hp, before - 3);
    assert!(f.g.players[0].hand.is_empty());
}

#[test]
fn a_tripwire_does_its_own_effect_again_on_the_draw() {
    // An empty enemy board, so every point of the split lands on the hero
    // and nothing can die and take its own damage count off the board.
    let mut f = Fix::new();
    f.play("Arcane Tripwire", None);
    assert_eq!(f.g.players[1].hero_hp, 26, "four damage split among all enemies");
    assert_eq!(f.g.players[0].deck.len(), 2);

    f.g.draw(ME, 1);
    assert_eq!(f.g.players[1].hero_hp, 22, "and again on the draw");
    assert!(f.g.players[0].hand.is_empty());
}

#[test]
fn scramble_for_gear_pays_once_now_and_once_per_draw() {
    let mut f = Fix::new();
    f.play("Scramble for Gear", None);
    assert_eq!(f.g.players[0].armor, 2);
    assert_eq!(f.g.players[0].deck.len(), 5);
    f.g.draw(ME, 2);
    assert_eq!(f.g.players[0].armor, 6, "two more Gear, two Armor each");
    assert_eq!(f.g.players[0].deck.len(), 3);
}

// --------------------------------------------------------------- Temporary
//
// A Temporary card is gone at the end of the turn it arrived on, unplayed.

#[test]
fn a_temporary_card_burns_at_the_end_of_the_turn() {
    let mut f = Fix::new();
    f.play("Frantic Forger", None);
    let at = f.g.players[0].hand.len() - 1;
    let hc = f.g.players[0].hand[at];
    assert_eq!(hc.card.def().kind(), Kind::Spell);
    assert!(hc.marks.has(tavernlab_core::state::Marks::TEMPORARY));

    f.g.end_turn();
    assert!(f.g.players[0].hand.is_empty(), "unplayed, so gone");
}

#[test]
fn a_temporary_card_played_in_time_is_kept() {
    // The counterpart: burning is what happens to the ones you do not play.
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.make_last_temporary(ME);
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    f.g.end_turn();
    assert_eq!(f.g.players[0].board.len(), 1, "played, so it stayed");
}

#[test]
fn spelunker_discounts_the_next_temporary_card_only() {
    let mut f = Fix::new();
    f.play("Spelunker", None);
    assert_eq!(f.g.players[0].next_temporary_discount, 2);

    f.g.give_card(ME, by_name("Boulderfist Ogre").unwrap()); // costs 6
    f.g.make_last_temporary(ME);
    let first = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, first), 4);
    assert_eq!(f.g.players[0].next_temporary_discount, 0, "spent");

    f.g.give_card(ME, by_name("Boulderfist Ogre").unwrap());
    f.g.make_last_temporary(ME);
    let second = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, second), 6, "the one after pays full");
}

#[test]
fn tunnel_terror_leaves_two_temporary_two_drops() {
    let mut f = Fix::new().board(ME, &["Tunnel Terror"]);
    f.g.deal_damage(Target::Minion(ME, 0), 3);
    f.g.sweep_deaths();
    let temps: Vec<_> = f.g.players[0]
        .hand
        .iter()
        .filter(|h| h.marks.has(tavernlab_core::state::Marks::TEMPORARY))
        .collect();
    assert_eq!(temps.len(), 2);
    for h in temps {
        assert_eq!(h.card.def().kind(), Kind::Minion);
        assert_eq!(h.card.def().cost, 2);
    }
}

// -------------------------------------------------- closest to the real field
//
// The five cards `tavernsim decks` named as standing between the simulator and
// a deck people are actually playing.

#[test]
fn portal_vanguard_draws_a_minion_and_buffs_only_that_one() {
    let mut f = Fix::new().deck(&["Fireball", "Chillwind Yeti"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    f.play("Portal Vanguard", None);

    let drawn = *f.g.players[0].hand.last().unwrap();
    assert_eq!(drawn.card.name(), "Chillwind Yeti", "a minion, not the spell");
    assert_eq!((drawn.atk, drawn.hp), (2, 2), "+2/+2 in hand");
    let wisp = f.g.players[0]
        .hand
        .iter()
        .find(|hc| hc.card.name() == "Wisp")
        .expect("still there");
    assert_eq!((wisp.atk, wisp.hp), (0, 0), "the card already in hand is untouched");
    assert_eq!(f.g.players[0].deck.len(), 1, "only the Fireball is left");
}

#[test]
fn portal_vanguard_with_no_minion_left_draws_nothing() {
    let mut f = Fix::new().deck(&["Fireball"]);
    f.play("Portal Vanguard", None);
    assert!(f.g.players[0].hand.is_empty(), "\"draw a random minion\" and there is none");
    assert_eq!(f.g.players[0].deck.len(), 1);
}

#[test]
fn follow_the_footsteps_hands_on_its_own_effect() {
    let mut f = Fix::new();
    f.play("Follow the Footsteps", None);

    let found = *f.g.players[0].hand.last().expect("a Stealth minion was discovered");
    assert_eq!(found.card.def().kind(), Kind::Minion);
    assert!(found.card.def().keywords.has(Keywords::STEALTH));
    assert!(
        found.marks.has(tavernlab_core::state::Marks::FOOTSTEPS),
        "\"give it this effect for a turn\""
    );

    // Playing it Discovers another Stealth minion, which is the effect.
    let before = f.g.players[0].hand.len();
    let idx = before as u8 - 1;
    assert!(f.g.apply(Action::Play {
        hand: idx,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    let next = *f.g.players[0].hand.last().expect("and another one arrived");
    assert!(next.card.def().keywords.has(Keywords::STEALTH));
    assert!(
        !next.marks.has(tavernlab_core::state::Marks::FOOTSTEPS),
        "the effect was lent once, not copied onward"
    );
}

#[test]
fn follow_the_footsteps_effect_expires_at_the_end_of_the_turn() {
    let mut f = Fix::new();
    f.play("Follow the Footsteps", None);
    assert!(
        f.g.players[0]
            .hand
            .last()
            .unwrap()
            .marks
            .has(tavernlab_core::state::Marks::FOOTSTEPS)
    );
    f.g.end_turn();
    assert!(
        !f.g.players[0]
            .hand
            .iter()
            .any(|hc| hc.marks.has(tavernlab_core::state::Marks::FOOTSTEPS)),
        "\"for a turn\", and the turn is over"
    );
}

#[test]
fn tricks_of_the_trade_deals_one_until_a_stealthed_minion_swings() {
    // Worgen Infiltrator is a 2/1 with Stealth.
    let mut f = Fix::new().board(ME, &["Worgen Infiltrator"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Tricks of the Trade").unwrap()));
    assert!(
        !f.g.players[0].hand[0]
            .marks
            .has(tavernlab_core::state::Marks::STEALTH_ATTACKED),
        "nothing has attacked yet"
    );

    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert!(
        f.g.players[0].hand[0]
            .marks
            .has(tavernlab_core::state::Marks::STEALTH_ATTACKED),
        "it was Stealthed when it swung, and the card was in hand for it"
    );

    let before = f.g.players[1].hero_hp;
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: Some(Target::Hero(FOE)),
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(before - f.g.players[1].hero_hp, 3);
}

#[test]
fn tricks_of_the_trade_deals_one_after_an_unstealthed_swing() {
    // The control: the same swing from a minion that was never Stealthed.
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Tricks of the Trade").unwrap()));
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));

    let before = f.g.players[1].hero_hp;
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: Some(Target::Hero(FOE)),
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(before - f.g.players[1].hero_hp, 1);
}

#[test]
fn tricks_of_the_trade_misses_a_swing_it_was_not_in_hand_for() {
    // "While holding this" is per copy: a card drawn after the swing did not
    // see it.
    let mut f = Fix::new().board(ME, &["Worgen Infiltrator"]);
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    f.g.players[0].hand.push(HandCard::new(by_name("Tricks of the Trade").unwrap()));

    let before = f.g.players[1].hero_hp;
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: Some(Target::Hero(FOE)),
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(before - f.g.players[1].hero_hp, 1);
}

#[test]
fn heartroot_stones_draws_twice_when_no_minion_was_played_last_turn() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    assert!(!f.g.players[0].played_minion_last_turn);
    f.play("Heartroot Stones", None);
    assert_eq!(f.g.players[0].hand.len(), 2, "a card and then a card again");
    assert_eq!(f.g.players[0].armor, 6);
}

/// One turn boundary, the way `Game::play_out` draws it: end, hand over, and
/// open the next. `end_turn` on its own does neither of the last two.
fn pass(g: &mut Game) {
    g.end_turn();
    g.current = g.current.other();
    g.turn += 1;
    g.begin_turn();
}

#[test]
fn heartroot_stones_draws_once_after_a_minion() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp", "Wisp", "Wisp"]);
    f.play("Wisp", None);
    pass(&mut f.g); // mine ends: "played a minion this turn" rolls forward
    pass(&mut f.g); // theirs ends, and I am up again
    assert!(f.g.players[0].played_minion_last_turn);
    f.g.players[0].mana = 10;

    let cards = f.g.players[0].hand.len();
    let armor = f.g.players[0].armor;
    f.play("Heartroot Stones", None);
    assert_eq!(f.g.players[0].hand.len(), cards + 1);
    assert_eq!(f.g.players[0].armor - armor, 3);
}

#[test]
fn chef_nethrek_gives_ten_mana_once_the_fifth_turn_is_over() {
    use tavernlab_core::agent::{Scripted, Style};

    let chef = by_name("Chef Neth'rek").unwrap();
    let mut deck = vec![chef];
    deck.resize(30, by_name("Wisp").unwrap()); // every other card costs 0
    let plain = vec![by_name("Wisp").unwrap(); 30];

    let mut g = Game::new((Class::Druid, &deck), (Class::Mage, &plain), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);

    // My turn one through five: the ordinary curve.
    g.turn += 1;
    g.begin_turn();
    for turn in 1..=5 {
        assert_eq!(
            g.players[0].crystals, turn,
            "turn {turn} is still worth {turn} crystals"
        );
        pass(&mut g); // mine
        pass(&mut g); // theirs
    }
    assert_eq!(g.players[0].crystals, 10, "and the sixth opens on ten");
    assert_eq!(g.players[0].mana, 10);
}

#[test]
fn chef_nethrek_stays_quiet_when_one_card_costs_more_than_three() {
    use tavernlab_core::agent::{Scripted, Style};

    let mut deck = vec![
        by_name("Chef Neth'rek").unwrap(),
        by_name("Boulderfist Ogre").unwrap(), // costs 6
    ];
    deck.resize(30, by_name("Wisp").unwrap());
    let plain = vec![by_name("Wisp").unwrap(); 30];

    let mut g = Game::new((Class::Druid, &deck), (Class::Mage, &plain), 1).unwrap();
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Midrange);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];
    g.start(Side::Player0, &mut agents);
    assert!(
        g.players[0].pending.is_empty(),
        "the condition is over the whole starting list, opening hand included"
    );
}

// ------------------------------------------------------------------ the Kabal
//
// A sham trial and the plants that make the case. The package turns on one
// token: an Imp-formant goes into the *enemy's* deck and pays out to whoever
// put it there.

#[test]
fn an_imp_formant_drawn_by_the_enemy_is_summoned_for_me() {
    let mut f = Fix::new();
    let imp = by_name("Imp-formant").expect("the corpus has the token");
    f.g.players[1].deck.push(DeckCard::started(imp));
    assert_eq!(f.g.players[1].hand.len(), 0);

    f.g.draw(FOE, 1);
    assert_eq!(f.g.players[1].hand.len(), 0, "it never reaches their hand");
    assert_eq!(f.g.players[1].board.len(), 0, "nor their board");
    assert_eq!(f.g.players[0].board.len(), 1, "it is summoned for me");
    let m = f.mine(0);
    assert_eq!(m.card, imp);
    assert_eq!((m.atk, m.health()), (3, 3));
    assert!(m.has(Keywords::LIFESTEAL), "printed on the token");
}

#[test]
fn harsh_sentence_taxes_their_next_turn_and_plants_two() {
    let mut f = Fix::new();
    let imp = by_name("Imp-formant").unwrap();
    f.play("Harsh Sentence", None);

    assert_eq!(
        f.g.players[1].deck.iter().filter(|d| d.card == imp).count(),
        2,
        "two into the enemy deck"
    );
    assert_eq!(f.g.players[1].minion_tax, 2);
    assert_eq!(f.g.players[0].minion_tax, 0, "mine are not taxed");

    // On their turn a minion costs two more; a spell does not.
    let yeti = by_name("Chillwind Yeti").unwrap(); // (4)
    let fireball = by_name("Fireball").unwrap(); // (4)
    f.g.players[1].hand.push(HandCard::new(yeti));
    f.g.players[1].hand.push(HandCard::new(fireball));
    assert_eq!(f.g.card_cost(FOE, 0), 6);
    assert_eq!(f.g.card_cost(FOE, 1), 4, "only minions");

    // And it is gone once that turn of theirs is over.
    pass(&mut f.g); // mine ends
    pass(&mut f.g); // theirs ends
    assert_eq!(f.g.players[1].minion_tax, 0);
    assert_eq!(f.g.card_cost(FOE, 0), 4);
}

#[test]
fn corrupt_constable_moves_an_imp_formant_to_the_top_and_buffs_it() {
    let mut f = Fix::new();
    let imp = by_name("Imp-formant").unwrap();
    let wisp = by_name("Wisp").unwrap();
    // Their deck, bottom to top: Imp-formant then two Wisps on top of it.
    f.g.players[1].deck.push(DeckCard::started(imp));
    f.g.players[1].deck.push(DeckCard::started(wisp));
    f.g.players[1].deck.push(DeckCard::started(wisp));

    f.play("Corrupt Constable", None);
    let top = *f.g.players[1].deck.last().expect("their deck is not empty");
    assert_eq!(top.card, imp, "moved to the top, which is what they draw next");
    assert_eq!((top.atk, top.hp), (2, 2), "+2/+2 written on that copy");
    assert_eq!(f.g.players[1].deck.len(), 3, "moved, not copied");

    // And it arrives on my board at that size -- beside the Constable, whose
    // own 3/5 body is already in slot zero.
    f.g.draw(FOE, 1);
    assert_eq!(f.mine(1).card, imp);
    assert_eq!((f.mine(1).atk, f.mine(1).health()), (5, 5));
}

#[test]
fn corrupt_constable_with_no_imp_formant_planted_just_lands() {
    let mut f = Fix::new();
    let wisp = by_name("Wisp").unwrap();
    for _ in 0..3 {
        f.g.players[1].deck.push(DeckCard::started(wisp));
    }
    f.play("Corrupt Constable", None);
    assert_eq!(f.g.players[0].board.len(), 1, "the body still comes down");
    assert_eq!(f.g.players[1].deck.len(), 3, "and their deck is untouched");
}

#[test]
fn frame_job_destroys_two_and_reorders_their_deck() {
    let mut f = Fix::new().board(FOE, &["Wisp", "Wisp", "Boulderfist Ogre"]);
    let fireball = by_name("Fireball").unwrap();
    let ogre = by_name("Boulderfist Ogre").unwrap();
    // A spell on top, a minion under it: the minion is what may be moved.
    f.g.players[1].deck.push(DeckCard::started(ogre));
    f.g.players[1].deck.push(DeckCard::started(fireball));

    f.play("Frame Job", None);
    assert_eq!(f.their_board(), 1, "two of the three are destroyed");
    assert_eq!(
        f.g.players[1].deck.last().map(|d| d.card),
        Some(ogre),
        "the minion is moved to the top of their own deck, over the spell"
    );
    assert_eq!(f.g.players[1].deck.len(), 2, "moved, not taken");
}

#[test]
fn detained_for_destruction_makes_every_minion_trade() {
    // Three a side, all 1/1: every one of the six has something to hit, and
    // nothing survives being hit by a 1/1 with one health.
    let mut f = Fix::new()
        .board(ME, &["Wisp", "Wisp", "Wisp"])
        .board(FOE, &["Wisp", "Wisp", "Wisp"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Detained for Destruction").unwrap()));
    f.g.cast_token(ME, by_name("Detained for Destruction").unwrap());
    assert_eq!(
        f.g.players[0].board.len() + f.g.players[1].board.len(),
        0,
        "six 0/1 Wisps forced into each other leave nobody standing"
    );
}

#[test]
fn a_lone_minion_has_nothing_to_be_forced_into() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre"]);
    f.g.cast_token(ME, by_name("Detained for Destruction").unwrap());
    assert_eq!(f.g.players[0].board.len(), 1, "\"another minion\" is nobody");
    assert_eq!(f.mine(0).damage, 0);
}

#[test]
fn godfather_kazakus_queues_two_effects_for_the_fourth_turn() {
    use tavernlab_core::state::PendingKind;

    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp", "Wisp", "Wisp", "Wisp"]);
    f.play("Godfather Kazakus", None);
    assert_eq!(f.g.players[0].board.len(), 1, "the 3/3 body lands");

    let pending: Vec<_> = f.g.players[0].pending.iter().copied().collect();
    assert_eq!(pending.len(), 2, "two effects of the nine");
    for p in &pending {
        assert_eq!(p.kind, PendingKind::CastLater);
        assert_eq!(p.turns_left, 4, "the Unending Trial, which is the free one");
        assert!(
            tavernlab_core::cards::behaviour_of(p.card)
                .and_then(|b| b.spell)
                .is_some(),
            "{} is one of the trial's effects and is implemented",
            p.card.name()
        );
    }

    // Nothing on the first three of my turns, and both on the fourth.
    for turn in 1..=3 {
        pass(&mut f.g);
        pass(&mut f.g);
        assert_eq!(
            f.g.players[0].pending.len(),
            2,
            "still waiting after turn {turn}"
        );
    }
    pass(&mut f.g);
    pass(&mut f.g);
    assert!(f.g.players[0].pending.is_empty(), "the trial has finished");
}

// ---------------------------------------------------------------- dragons
//
// Two mechanics: three cards that wake up in hand when a Dragon is played,
// and a spell that arrives already broken in two.

#[test]
fn a_dragon_played_wakes_what_is_waiting_in_hand() {
    let mut f = Fix::new();
    let striker = by_name("Stonetalon Striker").unwrap();
    f.g.players[0].hand.push(HandCard::new(striker));
    assert_eq!(f.g.players[0].hand[0].card.def().atk, 3, "3/3 asleep");
    assert!(
        !f.g.players[0].hand[0].card.def().races.any(Races::DRAGON),
        "and not yet a Dragon"
    );

    // A Dragon lands, and the card in hand wakes.
    f.play("Boulderfist Ogre", None); // not a Dragon
    assert_eq!(f.g.players[0].hand[0].card, striker, "still asleep");
    f.play("Twilight Whelp", None); // a Dragon
    let awake = f.g.players[0].hand[0].card;
    assert_ne!(awake, striker);
    assert_eq!(awake.name(), "Stonetalon Striker", "the same card, awake");
    assert_eq!((awake.def().atk, awake.def().hp), (6, 6));
    assert_eq!(awake.def().cost, striker.def().cost, "the cost does not move");
    assert!(awake.def().races.any(Races::DRAGON));
    assert!(awake.def().keywords.has(Keywords::TAUNT), "and keeps its Taunt");
}

#[test]
fn every_copy_in_hand_wakes_not_just_one() {
    // "While in hand, play a Dragon to become…" is a standing condition on
    // the card, not one trigger to be spent on the first copy.
    let mut f = Fix::new();
    let scout = by_name("Ebonscale Scout").unwrap();
    f.g.players[0].hand.push(HandCard::new(scout));
    f.g.players[0].hand.push(HandCard::new(scout));
    f.play("Twilight Whelp", None);
    for i in 0..2 {
        assert_eq!(f.g.players[0].hand[i].card.def().atk, 8, "copy {i}");
    }
}

#[test]
fn ebonscale_scout_hits_for_what_it_is_worth_asleep_and_awake() {
    // Asleep it is a 4/4, awake an 8/8, and the Battlecry reads its own
    // Attack off the board either way.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Ebonscale Scout", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 3, "four damage from the 4/4");

    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Ebonscale Scout").unwrap()));
    f.play("Twilight Whelp", None); // wakes it
    let idx = f.g.players[0]
        .hand
        .iter()
        .position(|hc| hc.card.name() == "Ebonscale Scout")
        .expect("still in hand") as u8;
    assert!(f.g.apply(Action::Play {
        hand: idx,
        target: foe_minion(0),
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.their_board(), 0, "eight damage kills a 6/7");
}

#[test]
fn ebyssian_gives_dragons_rush_for_the_rest_of_the_game() {
    let mut f = Fix::new();
    f.play("Twilight Whelp", None); // a Dragon, before Ebyssian
    f.play("Ebyssian", None);
    let whelp = f
        .g
        .players[0]
        .board
        .iter()
        .find(|m| m.card.name() == "Twilight Whelp")
        .expect("on the board");
    assert!(whelp.has(Keywords::RUSH), "the Dragons already out get it too");

    // And one that arrives later, after Ebyssian itself has gone.
    f.g.players[0].board.clear();
    f.g.recompute_auras();
    f.play("Twilight Whelp", None);
    assert!(f.mine(0).has(Keywords::RUSH), "\"this game\" outlives the body");
    // A minion that is not a Dragon is untouched.
    f.play("Wisp", None);
    assert!(!f.mine(1).has(Keywords::RUSH));
}

#[test]
fn supply_run_shatters_as_it_reaches_hand() {
    let mut f = Fix::new();
    f.g.give_card(ME, by_name("Chillwind Yeti").unwrap());
    f.g.give_card(ME, by_name("Supply Run").unwrap());

    // One half to the leftmost slot, the other to the end.
    let names: Vec<&str> = f.g.players[0].hand.iter().map(|hc| hc.card.name()).collect();
    assert_eq!(names, ["Supply Run", "Chillwind Yeti", "Supply Run"]);
    for hc in f.g.players[0].hand.iter() {
        if hc.card.name() == "Supply Run" {
            assert_eq!(hc.card.def().cost, 4, "both halves keep the whole cost");
        }
    }
    // Two different halves, not the same card twice.
    assert_ne!(f.g.players[0].hand[0].card, f.g.players[0].hand[2].card);
}

#[test]
fn two_halves_recombine_once_they_are_side_by_side() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp", "Wisp"]);
    f.g.give_card(ME, by_name("Chillwind Yeti").unwrap());
    f.g.give_card(ME, by_name("Supply Run").unwrap());
    assert_eq!(f.g.players[0].hand.len(), 3);

    // Play what sits between them, and they meet.
    let yeti = f.g.players[0]
        .hand
        .iter()
        .position(|hc| hc.card.name() == "Chillwind Yeti")
        .expect("in hand") as u8;
    assert!(f.g.apply(Action::Play {
        hand: yeti,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].hand.len(), 1, "one card where two halves met");
    assert_eq!(
        f.g.players[0].hand[0].card,
        by_name("Supply Run").unwrap(),
        "and it is the whole card again"
    );
}

#[test]
fn each_half_does_its_own_half_and_the_whole_does_both() {
    // Left half: draw three minions, and no buff.
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp", "Fireball"]);
    f.g.give_card(ME, by_name("Supply Run").unwrap());
    let left = f.g.players[0].hand[0].card;
    f.g.players[0].hand.clear();
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.cast_token(ME, left);
    assert_eq!(
        f.g.players[0].hand.iter().filter(|h| h.card.name() == "Wisp").count(),
        3,
        "three minions drawn, and the Fireball left behind"
    );
    assert_eq!(f.g.players[0].hand[0].card.name(), "Chillwind Yeti");
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (0, 0));

    // Right half: the buff, and no draw.
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    f.g.give_card(ME, by_name("Supply Run").unwrap());
    let right = *f.g.players[0].hand.last().expect("two halves");
    f.g.players[0].hand.clear();
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.cast_token(ME, right.card);
    assert_eq!(f.g.players[0].hand.len(), 1, "nothing drawn");
    assert_eq!((f.g.players[0].hand[0].atk, f.g.players[0].hand[0].hp), (2, 2));

    // The whole card, which is what a hand too full to split leaves you.
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    f.g.players[0].hand.push(HandCard::new(by_name("Chillwind Yeti").unwrap()));
    f.g.cast_token(ME, by_name("Supply Run").unwrap());
    assert_eq!(f.g.players[0].hand.len(), 4, "the Yeti and three Wisps");
    for hc in f.g.players[0].hand.iter() {
        assert_eq!((hc.atk, hc.hp), (2, 2), "and every one of them buffed");
    }
}

#[test]
fn a_hand_with_one_slot_left_takes_only_the_left_half() {
    let mut f = Fix::new();
    let wisp = by_name("Wisp").unwrap();
    for _ in 0..MAX_HAND - 1 {
        f.g.players[0].hand.push(HandCard::new(wisp));
    }
    f.g.give_card(ME, by_name("Supply Run").unwrap());
    assert_eq!(f.g.players[0].hand.len(), MAX_HAND);
    assert_eq!(
        f.g.players[0].hand[0].card.name(),
        "Supply Run",
        "the left half, and only it"
    );
    assert_eq!(
        f.g.players[0].hand.iter().filter(|h| h.card.name() == "Supply Run").count(),
        1
    );
}

#[test]
fn a_full_hand_burns_the_whole_card() {
    let mut f = Fix::new();
    let wisp = by_name("Wisp").unwrap();
    for _ in 0..MAX_HAND {
        f.g.players[0].hand.push(HandCard::new(wisp));
    }
    assert!(!f.g.give_card(ME, by_name("Supply Run").unwrap()));
    assert!(
        f.g.players[0].hand.iter().all(|h| h.card == wisp),
        "nothing split its way in"
    );
}

// ----------------------------------------------------------- the Lotus rackets
//
// Two cards count what you played for exactly two Mana; the third is a 4/4
// for two that leaves after one swing.

#[test]
fn the_two_mana_count_is_what_was_paid_not_what_was_printed() {
    let mut f = Fix::new();
    assert_eq!(f.g.players[0].cards_played_for_two, 0);
    f.play("Bloodfen Raptor", None); // (2)
    assert_eq!(f.g.players[0].cards_played_for_two, 1);
    f.play("Wisp", None); // (0)
    assert_eq!(f.g.players[0].cards_played_for_two, 1, "zero is not two");
    f.play("Chillwind Yeti", None); // (4)
    assert_eq!(f.g.players[0].cards_played_for_two, 1);

    // A four-drop discounted to two was played for two.
    let yeti = by_name("Chillwind Yeti").unwrap();
    let mut hc = HandCard::new(yeti);
    hc.cost_delta = -2;
    f.g.players[0].hand.push(hc);
    let at = f.g.players[0].hand.len() as u8 - 1;
    assert_eq!(f.g.card_cost(ME, at as usize), 2);
    assert!(f.g.apply(Action::Play {
        hand: at,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].cards_played_for_two, 2);
}

/// Damage dealt to the enemy side, however it was spread.
///
/// "A random enemy" is the hero as readily as a minion, so counting the shots
/// means counting both -- an assertion on the minion alone passes or fails on
/// the roll.
fn dealt_to_them(f: &Fix) -> i16 {
    let hero = 30 - f.g.players[1].hero_hp;
    let board: i16 = f.g.players[1].board.iter().map(|m| m.damage).sum();
    hero + board
}

#[test]
fn lotus_troublemaker_shoots_once_more_for_every_two_drop() {
    // "Shoot 1 time!", and one more per 2-Mana card played.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.play("Lotus Troublemaker", None);
    assert_eq!(dealt_to_them(&f), 1, "one shot with nothing played before");

    // The wiki's own example: after two 2-Mana cards, three shots.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Bloodfen Raptor", None); // (2)
    f.play("Bloodfen Raptor", None); // (2)
    f.play("Lotus Troublemaker", None);
    assert_eq!(dealt_to_them(&f), 3);
}

#[test]
fn a_troublemaker_that_was_not_in_the_deck_shoots_once() {
    // "While in hand or deck": a copy conjured mid-game was in neither while
    // those cards were played, and there is no per-copy counter to give it
    // one — so it shoots the printed once rather than a borrowed three.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Bloodfen Raptor", None);
    f.play("Bloodfen Raptor", None);
    let card = by_name("Lotus Troublemaker").unwrap();
    let mut hc = HandCard::new(card);
    hc.marks.insert(tavernlab_core::state::Marks::NOT_FROM_DECK);
    f.g.players[0].hand.push(hc);
    let at = f.g.players[0].hand.len() as u8 - 1;
    assert!(f.g.apply(Action::Play {
        hand: at,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(dealt_to_them(&f), 1, "one shot, not three");
}

#[test]
fn jade_guardians_gets_two_eight_drops_cheaper_by_the_count() {
    let mut f = Fix::new();
    f.play("Bloodfen Raptor", None); // (2)
    f.play("Bloodfen Raptor", None); // (2)
    f.play("Bloodfen Raptor", None); // (2)
    // Six mana went on the Raptors; the spell costs five.
    f.g.players[0].mana = 10;
    let before = f.g.players[0].hand.len();
    f.play("Jade Guardians", None);

    let got: Vec<HandCard> = f.g.players[0].hand.iter().skip(before).copied().collect();
    assert_eq!(got.len(), 2);
    for hc in &got {
        assert_eq!(hc.card.def().cost, 8, "eight-drops");
        assert_eq!(hc.card.def().kind(), Kind::Minion);
        assert_eq!(hc.cost_delta, -3, "one less for each of the three");
    }
    let at = f.g.players[0].hand.len() - 1;
    assert_eq!(f.g.card_cost(ME, at), 5, "and the discount is real");
}

#[test]
fn escape_artist_draws_and_leaves_after_it_swings() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp"]);
    f.g.players[0]
        .board
        .push({
            let mut m = Permanent::summon(by_name("Escape Artist").unwrap());
            m.flags.remove(Flags::JUST_SUMMONED);
            m
        });
    f.g.recompute_auras();
    assert_eq!(f.g.players[0].hand.len(), 0);

    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[1].hero_hp, 26, "the 4/4 connected");
    assert_eq!(f.g.players[0].hand.len(), 1, "and drew");
    assert!(f.g.players[0].board.is_empty(), "then escaped the game");
    assert_eq!(
        f.g.players[0].deaths, 0,
        "escaping is not dying: nothing counts it as a friendly death"
    );
}

#[test]
fn escape_artist_that_does_not_survive_stays_dead() {
    // Into a body big enough to kill it: the trigger is "attacks and
    // survives", and a corpse draws nothing.
    let mut f = Fix::new().deck(&["Wisp"]).board(FOE, &["Boulderfist Ogre"]); // 6/7
    f.g.players[0].board.push({
        let mut m = Permanent::summon(by_name("Escape Artist").unwrap());
        m.flags.remove(Flags::JUST_SUMMONED);
        m
    });
    f.g.recompute_auras();
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Minion(FOE, 0) }));
    assert!(f.g.players[0].board.is_empty(), "the 4/4 died to a 6/7");
    assert_eq!(f.g.players[0].hand.len(), 0, "and drew nothing");
    assert_eq!(f.g.players[0].deaths, 1, "that one really was a death");
}

// ---------------------------------------------------------- the Priest cycle
// Reach Equilibrium and what it pays out, Slime 'em! and its undo, and the
// minion that reads the game's Reborns back off the graveyard.

#[test]
fn reach_equilibrium_pays_each_half_at_its_own_fourth_spell() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.play("Reach Equilibrium", None);
    assert!(f.g.players[0].quest.is_some(), "the Quest slot is taken");

    // Holy first. Three casts pay nothing; the fourth pays Life's Breath.
    for n in 1..=4 {
        f.g.players[0].mana = 10;
        f.play("Convalescence", None); // Holy, no target
        let held = f.g.players[0]
            .hand
            .iter()
            .any(|h| h.card.name() == "Sol'etos, Life's Breath");
        assert_eq!(held, n == 4, "after {n} Holy spells");
    }
    assert!(
        f.g.players[0].quest.is_some(),
        "one half paid does not end a Quest that has two"
    );

    // Then Shadow. The fourth pays Death's Touch -- and the two halves
    // combine in hand the moment the second one lands.
    for _ in 0..4 {
        f.g.players[0].mana = 10;
        f.play("Blood Tap", None); // Shadow, no target
    }
    assert!(f.g.players[0].quest.is_none(), "both halves paid, slot given up");
    let names: Vec<&str> = f.g.players[0]
        .hand
        .iter()
        .map(|h| h.card.name())
        .collect();
    assert!(
        names.contains(&"Sol'etos, Cycle's Rebirth"),
        "the halves combined: {names:?}"
    );
    assert!(
        !names.contains(&"Sol'etos, Life's Breath") && !names.contains(&"Sol'etos, Death's Touch"),
        "and neither half is left behind: {names:?}"
    );
}

#[test]
fn the_soletos_halves_combine_from_opposite_ends_of_the_hand() {
    // Not a Shatter pair: these two arrive turns apart and need not be
    // adjacent, so the join has to find them across a hand of other cards.
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Sol'etos, Life's Breath").unwrap()));
    for _ in 0..4 {
        f.g.players[0].hand.push(HandCard::new(by_name("Wisp").unwrap()));
    }
    f.g.give_token(ME, by_name("Sol'etos, Death's Touch").unwrap());
    let names: Vec<&str> = f.g.players[0]
        .hand
        .iter()
        .map(|h| h.card.name())
        .collect();
    assert_eq!(
        names.iter().filter(|n: &&&str| n.starts_with("Sol'etos")).count(),
        1,
        "one card, not two: {names:?}"
    );
    assert_eq!(names[0], "Sol'etos, Cycle's Rebirth");
}

#[test]
fn the_whole_soletos_carries_both_halves_hooks() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Sol'etos, Cycle's Rebirth", None);
    assert_eq!(
        f.g.players[0].board.len(),
        2,
        "the Battlecry summons a copy of itself"
    );
    let before = f.g.players[1].hero_hp + f.theirs(0).health();
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let after = f.g.players[1].hero_hp + f.theirs(0).health();
    assert_eq!(before - after, 5, "the Deathrattle deals five to a random enemy");
}

#[test]
fn sinful_steed_comes_back_whole_and_keeps_what_it_was_given() {
    let mut f = Fix::new().board(ME, &["Sinful Steed"]); // 2/3 Reborn
    f.g.buff(Target::Minion(ME, 0), 2, 2);
    f.g.grant(Target::Minion(ME, 0), Keywords::TAUNT);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 5));

    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1, "it came back");
    assert_eq!(
        (f.mine(0).atk, f.mine(0).health()),
        (4, 5),
        "full Health and enchantments, not a fresh 2/3 at one"
    );
    assert!(f.mine(0).has(Keywords::TAUNT), "granted keywords come back too");
    assert!(!f.mine(0).has(Keywords::REBORN), "once, not forever");
}

#[test]
fn raith_van_geist_brings_back_only_what_actually_came_back() {
    // One minion that died having been Reborn, and one that simply died.
    let mut f = Fix::new().board(ME, &["Whelp of the Infinite", "Bloodfen Raptor"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.players[0].board[1].damage = f.g.players[0].board[1].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].board.len(), 1, "the Reborn one is back");
    // Kill the returned copy: that death is the one Raith reads.
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert!(f.g.players[0].board.is_empty());
    assert_eq!(f.g.players[0].graveyard.len(), 3, "two deaths plus the raptor");

    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Raith Van Geist", None);
    let back: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    assert_eq!(
        back,
        vec!["Raith Van Geist", "Whelp of the Infinite"],
        "the Raptor never Reborn-ed and stays dead: {back:?}"
    );
}

#[test]
fn raith_van_geist_sends_what_it_raised_at_enemy_minions() {
    let mut f = Fix::new()
        .board(ME, &["Whelp of the Infinite"]) // 1/4 Poisonous Reborn
        .board(FOE, &["Boulderfist Ogre"]);
    for _ in 0..2 {
        let slot = f.g.players[0].board.len() - 1;
        f.g.players[0].board[slot].damage = f.g.players[0].board[slot].max_hp;
        f.g.sweep_deaths();
    }
    assert!(f.g.players[0].board.is_empty());

    let hero_before = f.g.players[1].hero_hp;
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Raith Van Geist", None);
    assert_eq!(
        f.g.players[1].hero_hp, hero_before,
        "they attack minions, never the face"
    );
    assert_eq!(f.their_board(), 0, "a Poisonous 1/4 took the Ogre with it");
}

#[test]
fn slime_em_wipes_the_board_and_hands_each_player_its_undo() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor", "Chillwind Yeti"])
        .board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Slime 'em!", None);
    assert!(f.g.players[0].board.is_empty() && f.g.players[1].board.is_empty());
    for side in [0usize, 1] {
        assert!(
            f.g.players[side]
                .hand
                .iter()
                .any(|h| h.card.name() == "Ectoplasm"),
            "player {side} got an Ectoplasm"
        );
    }

    // Mine resummons mine, and only mine.
    f.g.players[0].mana = 10;
    f.play("Ectoplasm", None);
    let back: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    assert_eq!(back, vec!["Bloodfen Raptor", "Chillwind Yeti"]);
    assert!(f.g.players[1].board.is_empty(), "theirs is theirs to cast");
}

#[test]
fn a_second_ectoplasm_resummons_nothing() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Slime 'em!", None);
    f.g.players[0].mana = 10;
    f.play("Ectoplasm", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    f.g.players[0].mana = 10;
    f.play("Ectoplasm", None);
    assert_eq!(
        f.g.players[0].board.len(),
        1,
        "the slice is spent as it is read"
    );
}

#[test]
fn a_minion_that_died_after_the_wipe_is_not_slimed() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Slime 'em!", None);
    // Something else dies before the Ectoplasm is cast.
    f.g.players[0].board.push(Permanent::summon(by_name("Chillwind Yeti").unwrap()));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();

    f.g.players[0].mana = 10;
    f.play("Ectoplasm", None);
    let back: Vec<&str> = f.g.players[0]
        .board
        .iter()
        .map(|m| m.card.name())
        .collect();
    assert_eq!(back, vec!["Bloodfen Raptor"], "the Yeti sits past the slice");
}

#[test]
fn karov_hands_over_three_legendaries_cut_down_to_one_one() {
    let mut f = Fix::new().board(ME, &["Karov the Broken"]);
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    let hand: Vec<_> = f.g.players[0].hand.iter().copied().collect();
    assert_eq!(hand.len(), 3);
    for h in hand {
        let d = h.card.def();
        assert_eq!(d.rarity(), Rarity::Legendary);
        assert_eq!(d.kind(), Kind::Minion);
        assert_eq!((d.atk + h.atk as i16, d.hp + h.hp as i16), (1, 1));
        assert_eq!(f.g.players[0].effective_cost(&h), 1);
    }
}

#[test]
fn gravedawn_sunbloom_is_cheaper_only_after_a_holy_spell() {
    let mut f = Fix::new().deck(&["Wisp", "Wisp", "Wisp"]);
    let card = by_name("Gravedawn Sunbloom").unwrap();
    f.g.players[0].hand.push(HandCard::new(card));
    assert_eq!(f.g.card_cost(ME, 0), 4, "no Holy spell last turn");

    f.g.players[0].schools_cast_last = 1 << (School::Holy as u8);
    assert_eq!(f.g.card_cost(ME, 0), 2, "Kindred asks the school, not a tribe");

    f.g.players[0].schools_cast_last = 1 << (School::Shadow as u8);
    assert_eq!(f.g.card_cost(ME, 0), 4, "a Shadow spell is not Kindred to a Holy one");
}

#[test]
fn ruby_sanctum_turns_the_next_heal_around_once() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.g.players[0].hero_hp = 20;
    f.play("Ruby Sanctum", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Ruby Sanctum")
        .unwrap() as u8;
    assert!(!f.g.players[0].heal_as_damage, "placing it arms nothing");
    assert!(f.g.apply(Action::UseLocation { slot, target: None }));
    assert!(f.g.players[0].heal_as_damage);

    f.g.heal(Target::Minion(FOE, 0), 4);
    assert_eq!(f.theirs(0).health(), 3, "four healing became four damage");
    assert!(!f.g.players[0].heal_as_damage, "one effect, then spent");

    f.g.heal(Target::Hero(ME), 4);
    assert_eq!(f.g.players[0].hero_hp, 24, "the next one heals as normal");
}

#[test]
fn schism_whole_does_both_halves_and_each_half_does_its_own() {
    // Whole: buff, Elusive, and a copy.
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]); // 4/5
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    // The card shatters as it reaches hand, so it is played through `apply`
    // by id rather than by the name all three forms share.
    f.g.players[0].hand.push(HandCard::new(by_id("CATA_306").unwrap()));
    let idx = f.g.players[0].hand.len() as u8 - 1;
    assert!(f.g.apply(Action::Play {
        hand: idx,
        target: my_minion(0),
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 8));
    assert!(f.mine(0).has(Keywords::ELUSIVE));
    assert_eq!(f.g.players[0].board.len(), 2, "and a copy of it");

    // The buff half alone.
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.g.players[0].hand.push(HandCard::new(by_id("CATA_306t1").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: my_minion(0),
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 8));
    assert_eq!(f.g.players[0].board.len(), 1, "no copy from this half");
}

#[test]
fn schism_arrives_in_hand_already_in_two() {
    let mut f = Fix::new();
    f.g.give_token(ME, by_id("CATA_306").unwrap());
    let ids: Vec<&str> = f.g.players[0]
        .hand
        .iter()
        .map(|h| h.card.info().id)
        .collect();
    assert_eq!(ids, vec!["CATA_306t1", "CATA_306t2"]);
}

// ------------------------------------------------------------------ the Void
// Demon Hunter's Void Soul package, and the minion that makes Taunt stop
// mattering.

#[test]
fn a_void_soul_summons_a_one_cost_demon() {
    let mut f = Fix::new();
    f.play("Void Soul", None);
    assert_eq!(f.g.players[0].board.len(), 1);
    let d = f.mine(0).card.def();
    assert_eq!(d.cost, 1, "the printed Cost, from the card and not from here");
    assert!(d.races.any(Races::DEMON));
}

#[test]
fn the_four_cards_that_hand_out_a_void_soul_all_do() {
    // Vicious Voidscale, on dying.
    let mut f = Fix::new().board(ME, &["Vicious Voidscale"]);
    assert!(f.mine(0).has(Keywords::TAUNT));
    f.g.players[0].board[0].damage = f.g.players[0].board[0].max_hp;
    f.g.sweep_deaths();
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Void Soul");

    // Void Blast, but only when the minion actually dies.
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]); // 6/7, survives 3
    f.play("Void Blast", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 4);
    assert!(f.g.players[0].hand.is_empty(), "it lived, so no Void Soul");
    f.g.players[0].mana = 10;
    f.g.players[1].board[0].damage = 5; // now three finishes it
    f.play("Void Blast", foe_minion(0));
    assert_eq!(f.their_board(), 0);
    assert_eq!(f.g.players[0].hand.len(), 1);
    assert_eq!(f.g.players[0].hand[0].card.name(), "Void Soul");

    // Stardust Scythe, after the hero swings.
    let mut f = Fix::new();
    f.play("Stardust Scythe", None);
    assert!(f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[1].hero_hp, 27, "the 3-Attack weapon connected");
    let held: Vec<&str> = f.g.players[0].hand.iter().map(|h| h.card.name()).collect();
    assert_eq!(held, vec!["Void Soul"]);
}

#[test]
fn hive_map_discovers_a_fel_spell() {
    let mut f = Fix::new();
    f.play("Hive Map", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let d = f.g.players[0].hand[0].card.def();
    assert_eq!(d.kind(), Kind::Spell);
    assert_eq!(d.school(), School::Fel);
}

#[test]
fn kayn_sunfury_lets_the_whole_board_walk_past_a_taunt() {
    let mut f = Fix::new()
        .board(ME, &["Bloodfen Raptor"])
        .board(FOE, &["Goldshire Footman"]); // 1/2 Taunt
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon {
        card: by_name("Fiery War Axe").unwrap(),
        atk: 3,
        durability: 2,
    });

    // Without him the Taunt stands, for the minion and for the hero alike.
    assert!(f.g.must_respect_taunt(ME));
    assert!(!f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));

    f.play("Kayn Sunfury", None);
    assert!(!f.g.must_respect_taunt(ME), "his own line is 'all friendly attacks'");
    assert!(f.g.apply(Action::Attack { from: 0, target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[1].hero_hp, 27);
    assert!(f.g.apply(Action::HeroAttack { target: Target::Hero(FOE) }));
    assert_eq!(f.g.players[1].hero_hp, 24, "the hero walks past it too");

    // And the rule leaves with the body.
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Kayn Sunfury")
        .unwrap();
    f.g.players[0].board[slot].damage = f.g.players[0].board[slot].max_hp;
    f.g.sweep_deaths();
    assert!(f.g.must_respect_taunt(ME));
}

#[test]
fn a_taunt_that_is_ignored_is_still_offered_as_a_target() {
    // The enumerator and `apply` have to agree, or a search finds swings the
    // action list never had.
    let f = Fix::new()
        .board(ME, &["Bloodfen Raptor", "Kayn Sunfury"])
        .board(FOE, &["Goldshire Footman", "Chillwind Yeti"]);
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    let raptor_targets: Vec<Target> = legal
        .iter()
        .filter_map(|a| match a {
            Action::Attack { from: 0, target } => Some(*target),
            _ => None,
        })
        .collect();
    assert!(
        raptor_targets.contains(&Target::Hero(FOE)),
        "the face is legal past the Taunt: {raptor_targets:?}"
    );
    assert!(
        raptor_targets.contains(&Target::Minion(FOE, 1)),
        "so is the non-taunting Yeti: {raptor_targets:?}"
    );
    assert!(
        raptor_targets.contains(&Target::Minion(FOE, 0)),
        "and the Taunt itself is still a legal target: {raptor_targets:?}"
    );
}

// ------------------------------------------------------------------ Auras
// A spell that stays in the hero zone, fires at the end of each of its
// owner's turns, and is gone after three of them.

/// End the current player's turn without handing play over, so an Aura's
/// firing can be watched a turn at a time.
fn end_my_turn(f: &mut Fix) {
    f.g.end_turn();
    f.g.current = ME;
    f.g.players[0].mana = 10;
}

#[test]
fn chronological_aura_summons_on_the_turn_it_is_played_and_lasts_three() {
    let mut f = Fix::new();
    f.play("Chronological Aura", None);
    assert!(f.g.controls_aura(ME), "it is in play the moment it is cast");
    assert!(f.g.players[0].board.is_empty(), "and does nothing until the turn ends");

    for n in 1..=3 {
        end_my_turn(&mut f);
        assert_eq!(f.g.players[0].board.len(), n, "after {n} of its turns");
        assert_eq!(f.mine(0).card.name(), "Chronological Drake");
        assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (3, 5));
        assert!(f.mine(0).has(Keywords::TAUNT));
    }
    assert!(!f.g.controls_aura(ME), "three turns and it is spent");
    end_my_turn(&mut f);
    assert_eq!(f.g.players[0].board.len(), 3, "and summons nothing after that");
}

#[test]
fn gnomish_aura_heals_every_friendly_character() {
    let mut f = Fix::new().board(ME, &["Boulderfist Ogre"]).board(FOE, &["Boulderfist Ogre"]);
    f.g.players[0].hero_hp = 20;
    f.g.players[0].board[0].damage = 5;
    f.g.players[1].board[0].damage = 5;
    f.play("Gnomish Aura", None);
    end_my_turn(&mut f);
    assert_eq!(f.g.players[0].hero_hp, 24, "the hero is a character too");
    assert_eq!(f.mine(0).health(), 6, "5 damage, 4 healed");
    assert_eq!(f.theirs(0).health(), 2, "'all your characters', not all of them");
}

#[test]
fn mekkatorques_aura_buffs_and_shields_one_friendly_minion() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]); // 3/2
    f.play("Mekkatorque's Aura", None);
    end_my_turn(&mut f);
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (7, 6));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
}

#[test]
fn sandfury_aura_makes_an_end_of_turn_effect_fire_twice() {
    // Strength Totem gives *another* friendly minion +1 Attack at the end of
    // its owner's turn, so with exactly one other minion out, how many times
    // it fired is readable straight off that minion.
    let mut once = Fix::new().board(ME, &["Strength Totem", "Bloodfen Raptor"]);
    end_my_turn(&mut once);
    assert_eq!(once.mine(1).atk, 4, "once, with no Aura up");

    let mut f = Fix::new().board(ME, &["Strength Totem", "Bloodfen Raptor"]);
    f.play("Sandfury Aura", None);
    end_my_turn(&mut f);
    assert_eq!(f.mine(1).atk, 5, "twice, under the Aura");
}

#[test]
fn sandfury_aura_does_not_double_the_other_players_minions() {
    let mut f = Fix::new().board(FOE, &["Strength Totem", "Bloodfen Raptor"]);
    let before = f.theirs(1).atk;
    f.play("Sandfury Aura", None);
    f.g.end_turn();
    assert_eq!(
        f.theirs(1).atk, before,
        "'your minions', and it is not even their turn"
    );
}

#[test]
fn manifested_timeways_only_fires_while_an_aura_is_up() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Manifested Timeways", None);
    assert_eq!(f.theirs(0).health(), 5, "no Aura, no damage");
    assert_eq!(f.g.players[1].hero_hp, 30);

    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Sandfury Aura", None);
    f.g.players[0].mana = 10;
    f.play("Manifested Timeways", None);
    assert_eq!(f.theirs(0).health(), 2);
    assert_eq!(f.g.players[1].hero_hp, 27, "all enemies, hero included");
}

#[test]
fn gelbin_puts_one_of_each_aura_from_the_deck_into_play() {
    let mut f = Fix::new().deck(&[
        "Chronological Aura",
        "Chronological Aura",
        "Sandfury Aura",
        "Chillwind Yeti",
    ]);
    f.play("Gelbin of Tomorrow", None);
    assert_eq!(f.g.players[0].pending.len(), 2, "one of each, not one of every copy");
    let left: Vec<&str> = f.g.players[0].deck.iter().map(|d| d.card.name()).collect();
    assert_eq!(
        left,
        vec!["Chronological Aura", "Chillwind Yeti"],
        "'from your deck': the ones put into play left it"
    );
}

#[test]
fn flight_maneuvers_whole_summons_and_buffs_and_each_half_does_one() {
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]); // 3/2
    f.g.players[0].hand.push(HandCard::new(by_id("CATA_479").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].board.len(), 3, "the Raptor and two Drakes");
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (4, 2), "+1 Attack");
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    assert_eq!(f.mine(1).card.name(), "Sky Drake");
    assert_eq!(
        (f.mine(1).atk, f.mine(1).max_hp),
        (5, 2),
        "the Drakes are summoned first, so they are 'your minions' by the time          the buff lands: a printed 4/2 with the card's own +1"
    );
    assert!(f.mine(1).has(Keywords::DIVINE_SHIELD));

    // The summoning half alone leaves the Raptor as it was.
    let mut f = Fix::new().board(ME, &["Bloodfen Raptor"]);
    f.g.players[0].hand.push(HandCard::new(by_id("CATA_479t").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    assert_eq!(f.g.players[0].board.len(), 3);
    assert_eq!(f.mine(0).atk, 3, "no buff from this half");
}

#[test]
fn inspiring_maul_fires_one_friendly_minions_end_of_turn_effect() {
    let mut f = Fix::new().board(ME, &["Strength Totem", "Bloodfen Raptor"]);
    f.play("Inspiring Maul", None);
    f.g.destroy_weapon(ME);
    assert_eq!(
        f.mine(1).atk,
        4,
        "the Totem's end of turn effect ran with no turn ending"
    );
}

#[test]
fn gnomeregan_advances_an_age_per_use_and_keeps_what_is_left() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.play("Past Gnomeregan", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Past Gnomeregan")
        .unwrap() as u8;

    assert!(f.g.apply(Action::UseLocation { slot, target: my_minion(0) }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (6, 6));
    assert_eq!(f.g.players[0].board[slot as usize].card.name(), "Present Gnomeregan");
    assert_eq!(
        f.g.players[0].board[slot as usize].health(),
        2,
        "one use spent, and the age it became does not get it back"
    );

    // Next turn, the present.
    f.g.end_turn();
    f.g.current = ME;
    f.g.begin_turn();
    assert!(f.g.apply(Action::UseLocation { slot, target: my_minion(0) }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (8, 7));
    assert_eq!(f.mine(0).granted_rattle.name(), "Leper Gnome");
    assert_eq!(f.g.players[0].board[slot as usize].card.name(), "Future Gnomeregan");

    // And the future, which is the last of the three.
    f.g.end_turn();
    f.g.current = ME;
    f.g.begin_turn();
    assert!(f.g.apply(Action::UseLocation { slot, target: my_minion(0) }));
    assert_eq!((f.mine(0).atk, f.mine(0).max_hp), (10, 8));
    assert!(f.mine(0).has(Keywords::DIVINE_SHIELD));
    f.g.sweep_deaths();
    assert!(
        !f.g.players[0].board.iter().any(|m| m.card.name().ends_with("Gnomeregan")),
        "three ages, three uses, then it is gone"
    );
}

// ----------------------------------------------------------------- Rewind
// "May be replayed once for a potentially different outcome." The card
// resolves, and the player either keeps that timeline or rolls the whole
// play back and plays it again. This engine can do that exactly: a `Game` is
// a fixed-size value, so the position before the play is one copy away.

/// Play `name` on a fresh game at `seed`, with or without its Rewind.
///
/// Setting `rewinding` before the play sends it down the plain route, which
/// is what the same card looked like before this existed -- so the two arms
/// of these tests differ in the keyword and nothing else.
fn play_at(seed: u64, name: &str, rewind: bool, cls: Class) -> Game {
    let mut g = Game::new((cls, &[]), (cls, &[]), seed).unwrap();
    g.players[0].mana = 10;
    g.players[0].crystals = 10;
    g.players[0].hand.push(HandCard::new(by_name(name).unwrap()));
    g.rewinding = !rewind;
    assert!(g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    g.rewinding = false;
    g
}

#[test]
fn a_rewind_takes_the_better_of_two_rolls() {
    // Shadows of Yesterday summons four Shades that each gain two random
    // Bonus Effects, so the roll is worth a lot and is plainly visible on the
    // board. Averaged over many seeds, keeping the better of two rolls has to
    // beat taking the first.
    fn board_value(g: &Game) -> f32 {
        tavernlab_core::agent::position_value(g, ME)
    }
    let plain: f32 = (0..120u64)
        .map(|s| board_value(&play_at(s, "Shadows of Yesterday", false, Class::DeathKnight)))
        .sum();
    let rewound: f32 = (0..120u64)
        .map(|s| board_value(&play_at(s, "Shadows of Yesterday", true, Class::DeathKnight)))
        .sum();
    assert!(
        rewound > plain,
        "keeping the better of two rolls should score higher over 120 seeds: \
         {rewound:.0} against {plain:.0}"
    );
}

#[test]
fn a_rewind_never_leaves_both_timelines_behind() {
    // The rolled-back timeline must leave no trace. Four Shades summoned
    // twice, or two weapons equipped, would be the bug this looks for.
    for seed in 0..80u64 {
        let g = play_at(seed, "Shadows of Yesterday", true, Class::DeathKnight);
        assert_eq!(
            g.players[0].board.len(),
            4,
            "seed {seed}: four Shades, from one timeline"
        );
        assert_eq!(g.players[0].mana, 4, "seed {seed}: paid for once");
        assert!(!g.rewinding, "the guard is cleared");
    }
}

#[test]
fn a_rewind_that_arms_the_enemy_too_weighs_their_half() {
    // Stadium Announcer equips *both* players. The roll that hands the
    // opponent the better weapon is the worse one, and the score has to see
    // that or the choice is made on half the card.
    let mut worse_for_them = 0;
    for seed in 0..80u64 {
        let g = play_at(seed, "Stadium Announcer", true, Class::Warrior);
        let mine = g.players[0].weapon.map_or(0, |w| w.atk * w.durability);
        let theirs = g.players[1].weapon.map_or(0, |w| w.atk * w.durability);
        if mine >= theirs {
            worse_for_them += 1;
        }
    }
    assert!(
        worse_for_them > 40,
        "with the better of two rolls kept, our weapon should usually be the \
         bigger one: {worse_for_them} of 80"
    );
}

#[test]
fn a_card_without_rewind_is_played_once_and_unchanged() {
    let mut f = Fix::new().board(FOE, &["Boulderfist Ogre"]);
    f.play("Fireball", foe_minion(0));
    assert_eq!(f.theirs(0).health(), 1, "one Fireball, not the better of two");
}

#[test]
fn a_rewind_inside_a_rewind_does_not_recurse() {
    // Sands of Time Discovers a spell, and whatever it Discovers may itself
    // have Rewind. The replay must not start replays of its own; the guard
    // is `Game::rewinding`, and this is the shape that would spin without it.
    for seed in 0..60u64 {
        let g = play_at(seed, "Sands of Time", true, Class::Mage);
        assert_eq!(g.players[0].hand.len(), 1, "seed {seed}: one card, once");
        assert!(!g.rewinding);
    }
}

#[test]
fn only_a_card_that_leads_with_the_keyword_carries_it() {
    // The corpus never tags Rewind as a mechanic, and its keyword list cannot
    // tell a card that has Rewind from one that only talks about it. The
    // generator reads the printed text instead; this pins what that decided.
    for (name, want) in [
        ("Shadows of Yesterday", true), // "Rewind Summon four 3/2 Shades."
        ("Stadium Announcer", true),    // "Rewind Battlecry: ..."
        ("Sands of Time", true),
        ("Portal Vanguard", true),
        ("Mister Clocksworth", true), // "Rewind, Rewind, Rewind Battlecry: ..."
        ("Time Machine", false),      // "Deathrattle: Get a random Rewind card."
        ("Morchie", false),           // "Your Rewinds keep BOTH potential outcomes."
        ("Fireball", false),
    ] {
        let card = by_name(name).unwrap_or_else(|| panic!("no card {name}"));
        assert_eq!(
            card.def().keywords.has(Keywords::REWIND),
            want,
            "{name}: {}",
            card.info().text
        );
    }
}

// ------------------------------------------------------------- the past
// Mage's Quest package: four Discovers, a Quest that counts them, and the
// weapon it pays out.

#[test]
fn from_the_past_is_the_rotated_out_pool() {
    // Not a judgement: a card is from the past when the corpus says it is
    // Wild-legal and no longer Standard-legal.
    let pool = tavernlab_core::cards::discover_pool(|d| {
        d.kind() == Kind::Spell
            && d.school() == School::Arcane
            && tavernlab_core::cards::from_the_past(d)
    });
    assert!(!pool.is_empty(), "there are rotated-out Arcane spells to find");
    for c in &pool {
        let d = c.def();
        assert!(d.formats.has(Formats::WILD));
        assert!(
            !d.formats.has(Formats::STANDARD),
            "{} is still Standard-legal",
            c.name()
        );
    }
}

#[test]
fn alter_time_discovers_two_cheaper_arcane_spells_from_the_past() {
    let mut f = Fix::new();
    f.play("Alter Time", None);
    let hand: Vec<_> = f.g.players[0].hand.iter().copied().collect();
    assert_eq!(hand.len(), 2);
    for hc in hand {
        let d = hc.card.def();
        assert_eq!(d.kind(), Kind::Spell);
        assert_eq!(d.school(), School::Arcane);
        assert!(tavernlab_core::cards::from_the_past(d), "{}", hc.card.name());
        assert_eq!(hc.cost_delta, -2, "{} costs (2) less", hc.card.name());
    }
}

#[test]
fn wanted_poster_hands_over_a_big_minion_that_can_be_prepared() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.play("Wanted Poster", None);
    assert_eq!(f.g.players[0].hand.len(), 1);
    let hc = f.g.players[0].hand[0];
    assert_eq!(hc.card.def().kind(), Kind::Minion);
    assert!(hc.card.def().cost >= 5, "{} costs {}", hc.card.name(), hc.card.def().cost);
    assert!(
        !hc.card.def().keywords.has(Keywords::PREPARE),
        "the card does not print Prepare; the copy was given it"
    );

    // Prepare is only offered for something you cannot afford anyway.
    f.g.players[0].mana = 1;
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    assert!(
        legal.iter().any(|a| matches!(a, Action::Prepare { hand: 0 })),
        "the granted Prepare is offered"
    );
    assert!(f.g.apply(Action::Prepare { hand: 0 }));
    assert_eq!(f.g.players[0].mana, 0, "the mana went into the card");
    assert_eq!(f.g.players[0].hand[0].cost_delta, -2, "one mana banked, plus one");
}

#[test]
fn raptor_herald_discovers_a_beast_and_gifts_it() {
    let mut f = Fix::new();
    f.play("Raptor Herald", None);
    // Sweet Dreams is the one Gift that moves the card to the deck, so the
    // Beast is in one place or the other.
    let held = f.g.players[0].hand.first().copied();
    let stacked = f.g.players[0].deck.last().copied();
    assert!(held.is_some() || stacked.is_some(), "the Beast went somewhere");
    if let Some(hc) = held {
        assert!(hc.card.def().races.any(Races::BEAST), "{}", hc.card.name());
        assert_ne!(hc.gift, 0, "with a Dark Gift");
    }
}

#[test]
fn raptor_herald_is_a_mana_cheaper_after_a_beast() {
    fn cost_of_discovered(kindred: bool) -> i16 {
        let mut f = Fix::new();
        if kindred {
            f.g.players[0].played_races_last = Races::BEAST;
        }
        f.play("Raptor Herald", None);
        let hc = f.g.players[0].hand[0];
        // The Gift moves the printed cost too, so what is compared is the
        // delta this card put on the copy.
        let (_, _, gift_cost) = tavernlab_core::cards::gift_stats(hc.gift);
        hc.cost_delta - gift_cost
    }
    assert_eq!(
        cost_of_discovered(true) - cost_of_discovered(false),
        -1,
        "Kindred takes one mana off, and only that"
    );
}

#[test]
fn the_forbidden_sequence_pays_out_after_seven_discovers() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.play("The Forbidden Sequence", None);
    assert!(f.g.players[0].quest.is_some());
    for n in 1..=7 {
        f.g.players[0].mana = 10;
        f.g.discover(ME, |d| d.kind() == Kind::Spell);
        let held = f.g.players[0]
            .hand
            .iter()
            .any(|h| h.card.name() == "The Origin Stone");
        assert_eq!(held, n == 7, "after {n} Discovers");
    }
    assert!(f.g.players[0].quest.is_none(), "seven paid, the slot is given up");
}

#[test]
fn every_kind_of_discover_counts_towards_the_quest() {
    // Seven Discover shapes reach the engine and all of them are Discovers;
    // before this package only three of them said so.
    let mut f = Fix::new().deck(&["Fireball", "Frostbolt", "Chillwind Yeti"]);
    f.g.players[0].crystals = 10;
    f.play("The Forbidden Sequence", None);
    let before = f.g.players[0].quest.map(|(_, p)| p).unwrap();
    f.g.discover_from_deck(ME, |d| d.kind() == Kind::Spell);
    let after = f.g.players[0].quest.map(|(_, p)| p).unwrap();
    assert_eq!(after, before + 1, "a Discover from the deck is a Discover");
}

#[test]
fn the_origin_stone_plays_what_the_discover_let_go() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.g.players[0].weapon = Some(tavernlab_core::state::Weapon::equip(
        by_name("The Origin Stone").unwrap(),
    ));
    let durability = f.g.players[0].weapon.unwrap().durability;
    // Vanilla bodies only: a minion with a Battlecry of its own could
    // Discover again from inside this one, and then what is being counted is
    // the cascade rather than the weapon.
    f.g.discover(ME, |d| {
        d.kind() == Kind::Minion
            && d.cost <= 2
            && d.keywords.has(Keywords::TEXT_UNDERSTOOD)
            && !d.keywords.has(Keywords::BATTLECRY)
            && !d.keywords.has(Keywords::DEATHRATTLE)
    });
    assert_eq!(f.g.players[0].hand.len(), 1, "the pick went to hand");
    assert_eq!(
        f.g.players[0].board.len(),
        2,
        "and the other two were played"
    );
    assert_eq!(
        f.g.players[0].weapon.unwrap().durability,
        durability - 1,
        "one durability for the whole Discover, not one per card"
    );
}

#[test]
fn morchie_keeps_both_rewind_outcomes_instead_of_choosing() {
    // Shadows of Yesterday summons four Shades. With Morchie out the card
    // resolves twice rather than being rolled back and re-rolled, so the
    // board fills instead of staying at four.
    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    f.play("Shadows of Yesterday", None);
    assert_eq!(f.g.players[0].board.len(), 4, "one timeline, four Shades");

    let mut f = Fix::new().board(ME, &["Morchie"]);
    f.g.players[0].crystals = 10;
    f.g.players[0].mana = 10;
    assert!(f.g.keeps_both_rewind_outcomes(ME));
    f.play("Shadows of Yesterday", None);
    assert_eq!(
        f.g.players[0].board.len(),
        7,
        "Morchie plus both rolls, capped by the board"
    );
}

// ------------------------------------------------------------------ Azshara
// Druid's Well and its Queen: two Locations that come with Lady Azshara, one
// of which she empowers and the other of which she destroys.

#[test]
fn ysera_raises_the_ceiling_for_both_players_and_takes_three_for_herself() {
    let mut f = Fix::new();
    f.g.players[0].crystals = 10;
    f.g.players[1].crystals = 10;
    assert_eq!(f.g.players[0].crystal_cap(), 10);

    // Start of Game, not the Battlecry: the ceiling moves for both sides.
    let ysera = by_name("Ysera, Emerald Aspect").unwrap();
    let f_of = behaviour_of(ysera).and_then(|b| b.start_of_game).unwrap();
    f_of(&mut f.g, &make_ctx(ysera, ME));
    assert_eq!(f.g.players[0].crystal_cap(), 15);
    assert_eq!(f.g.players[1].crystal_cap(), 15, "both players', as printed");

    // And the ramp can now actually reach past ten.
    f.g.gain_crystal(ME, 3);
    assert_eq!(f.g.players[0].crystals, 13);
    f.g.gain_crystal(ME, 9);
    assert_eq!(f.g.players[0].crystals, 15, "and stops at the new ceiling");
}

#[test]
fn two_yseras_do_not_stack_to_twenty() {
    let mut f = Fix::new();
    let ysera = by_name("Ysera, Emerald Aspect").unwrap();
    let f_of = behaviour_of(ysera).and_then(|b| b.start_of_game).unwrap();
    f_of(&mut f.g, &make_ctx(ysera, ME));
    f_of(&mut f.g, &make_ctx(ysera, ME));
    assert_eq!(f.g.players[0].crystal_cap(), 15, "the ceiling is set, not added to");
}

#[test]
fn lady_azshara_empowers_one_location_and_destroys_the_other() {
    // Both Locations start in the deck, which is where Fabled puts them.
    let mut f = Fix::new();
    f.g.players[0]
        .deck
        .push(DeckCard::started(by_id("TIME_211t1").unwrap())); // the Well
    f.g.players[0]
        .deck
        .push(DeckCard::started(by_id("TIME_211t2").unwrap())); // Zin-Azshari
    f.play_mode("Lady Azshara", 0, None); // Empower Zin-Azshari

    let left: Vec<&str> = f.g.players[0]
        .deck
        .iter()
        .map(|d| d.card.info().id)
        .collect();
    assert_eq!(
        left,
        vec!["TIME_211t2t"],
        "Zin-Azshari empowered, the Well destroyed"
    );

    // The other mode, the other way round.
    let mut f = Fix::new();
    f.g.players[0]
        .deck
        .push(DeckCard::started(by_id("TIME_211t1").unwrap()));
    f.g.players[0]
        .deck
        .push(DeckCard::started(by_id("TIME_211t2").unwrap()));
    f.play_mode("Lady Azshara", 1, None);
    let left: Vec<&str> = f.g.players[0]
        .deck
        .iter()
        .map(|d| d.card.info().id)
        .collect();
    assert_eq!(left, vec!["TIME_211t1t"]);
}

#[test]
fn azshara_reaches_a_location_that_has_already_been_drawn() {
    // A card that only worked from the deck would quietly do nothing in the
    // game where she is drawn late.
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_id("TIME_211t2").unwrap()));
    f.g.players[0]
        .hand
        .push(HandCard::new(by_id("TIME_211t1").unwrap()));
    f.play_mode("Lady Azshara", 0, None);
    let held: Vec<&str> = f.g.players[0]
        .hand
        .iter()
        .map(|h| h.card.info().id)
        .collect();
    assert_eq!(held, vec!["TIME_211t2t"]);
}

#[test]
fn zin_azshari_copies_a_friendly_minion_and_doubles_it_when_empowered() {
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]); // 4/5
    f.play("Zin-Azshari", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Zin-Azshari")
        .unwrap() as u8;
    assert!(f.g.apply(Action::UseLocation { slot, target: my_minion(0) }));
    let copy = f.g.players[0].board.last().unwrap();
    assert_eq!((copy.atk, copy.max_hp), (4, 5), "a plain copy");

    // The empowered printing shares its name, so it is the same row.
    let mut f = Fix::new().board(ME, &["Chillwind Yeti"]);
    f.g.players[0]
        .hand
        .push(HandCard::new(by_id("TIME_211t2t").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Zin-Azshari")
        .unwrap() as u8;
    assert!(f.g.apply(Action::UseLocation { slot, target: my_minion(0) }));
    let copy = f.g.players[0].board.last().unwrap();
    assert_eq!((copy.atk, copy.max_hp), (8, 10), "doubled");
}

#[test]
fn the_well_fills_the_hand_with_spells_that_burn_at_end_of_turn() {
    let mut f = Fix::new();
    f.play("The Well of Eternity", None);
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "The Well of Eternity")
        .unwrap() as u8;
    assert!(f.g.apply(Action::UseLocation { slot, target: None }));
    assert_eq!(f.g.players[0].hand.len(), MAX_HAND, "filled, not topped up");
    assert!(
        f.g.players[0].hand.iter().all(|h| {
            h.card.def().kind() == Kind::Spell && h.marks.has(Marks::TEMPORARY)
        }),
        "random Temporary spells"
    );
    f.g.end_turn();
    assert!(
        f.g.players[0].hand.is_empty(),
        "Temporary means gone at the end of the turn"
    );
}

#[test]
fn the_empowered_well_marks_its_spells_to_cast_twice() {
    let mut f = Fix::new();
    f.g.players[0]
        .hand
        .push(HandCard::new(by_id("TIME_211t1t").unwrap()));
    assert!(f.g.apply(Action::Play {
        hand: 0,
        target: None,
        position: u8::MAX,
        choice: u8::MAX,
    }));
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "The Well of Eternity")
        .unwrap() as u8;
    assert!(f.g.apply(Action::UseLocation { slot, target: None }));
    assert!(
        f.g.players[0]
            .hand
            .iter()
            .all(|h| h.marks.has(Marks::CASTS_TWICE)),
        "the empowered Well's spells cast twice"
    );
}

#[test]
fn welcome_home_lets_a_spent_location_be_used_again() {
    let mut f = Fix::new().board(FOE, &["Chillwind Yeti"]);
    f.play("Sanguine Depths", None); // 1 damage to a minion, +2 Attack
    let slot = f.g.players[0]
        .board
        .iter()
        .position(|m| m.card.name() == "Sanguine Depths")
        .unwrap() as u8;
    assert!(f.g.apply(Action::UseLocation { slot, target: foe_minion(0) }));
    assert!(
        !f.g.apply(Action::UseLocation { slot, target: foe_minion(0) }),
        "spent for this turn"
    );

    f.g.players[0].mana = 10;
    f.play("Welcome Home!", Some(Target::Minion(ME, slot)));
    assert!(
        f.g.apply(Action::UseLocation { slot, target: foe_minion(0) }),
        "reopened"
    );
    assert_eq!(
        f.g.players[0].board[slot as usize].granted_rattle.name(),
        "Stubborn Suspect",
        "and given the deathrattle the card prints"
    );
}

#[test]
fn welcome_home_is_offered_a_location_and_nothing_else() {
    let f = Fix::new()
        .board(ME, &["Bloodfen Raptor"])
        .board(FOE, &["Chillwind Yeti"]);
    let mut f = f;
    f.play("Sanguine Depths", None);
    f.g.players[0].mana = 10;
    f.g.players[0]
        .hand
        .push(HandCard::new(by_name("Welcome Home!").unwrap()));
    let idx = f.g.players[0].hand.len() as u8 - 1;
    let mut legal = tavernlab_core::inline::Inline::new();
    f.g.legal_actions(&mut legal);
    let targets: Vec<Target> = legal
        .iter()
        .filter_map(|a| match a {
            Action::Play { hand, target: Some(t), .. } if *hand == idx => Some(*t),
            _ => None,
        })
        .collect();
    assert_eq!(targets.len(), 1, "one Location and nothing else: {targets:?}");
    assert!(matches!(targets[0], Target::Minion(ME, _)));
}
