"""Deck definitions.

META: canonical documented top ladder decks for this card pool (the
"deck tracker" gauntlet). CANDIDATES: our seed decks (2 per class) that the
optimizer refines. POOLS: per-class swap candidates for optimization."""
from .cards import DB, get_card


class Deck:
    def __init__(self, name, cls, archetype, cardlist):
        self.name = name
        self.cls = cls
        self.archetype = archetype
        self.cardlist = list(cardlist)  # [(name, count)]
        total = sum(n for _, n in cardlist)
        assert total == 30, f"{name}: {total} cards"
        for cn, n in cardlist:
            assert cn in DB, f"{name}: unknown card {cn}"
            assert 1 <= n <= 2, f"{name}: bad count {cn}"
            assert not DB[cn].token, f"{name}: token {cn}"
            c = DB[cn]
            assert c.cls in (None, cls), f"{name}: {cn} is {c.cls}"

    @property
    def cards(self):
        out = []
        for cn, n in self.cardlist:
            out.extend([get_card(cn)] * n)
        return out

    def copy_with_swap(self, out_name, in_name):
        cl = dict(self.cardlist)
        cl[out_name] -= 1
        if cl[out_name] == 0:
            del cl[out_name]
        cl[in_name] = cl.get(in_name, 0) + 1
        return Deck(self.name, self.cls, self.archetype, sorted(cl.items()))


META = [
    Deck("Tempo Mage [meta]", "Mage", "midrange", [
        ("Mana Wyrm", 2), ("Frostbolt", 2), ("Sorcerer's Apprentice", 2),
        ("Mad Scientist", 2), ("Mirror Entity", 1), ("Counterspell", 1),
        ("Arcane Intellect", 2), ("Fireball", 2), ("Polymorph", 1),
        ("Water Elemental", 2), ("Azure Drake", 2), ("Chillwind Yeti", 2),
        ("Flamestrike", 2), ("Archmage Antonidas", 1),
        ("Arcane Missiles", 2), ("Knife Juggler", 2), ("Pyroblast", 1),
        ("Loot Hoarder", 1)]),
    Deck("Midrange Hunter [meta]", "Hunter", "midrange", [
        ("Webspinner", 2), ("Haunted Creeper", 2), ("Mad Scientist", 2),
        ("Freezing Trap", 1), ("Explosive Trap", 1),
        ("Animal Companion", 2), ("Kill Command", 2), ("Eaglehorn Bow", 2),
        ("Unleash the Hounds", 2), ("Houndmaster", 2),
        ("Piloted Shredder", 2), ("Savannah Highmane", 2),
        ("Sludge Belcher", 1), ("Dr. Boom", 1), ("Knife Juggler", 2),
        ("Dire Wolf Alpha", 2), ("Leper Gnome", 2)]),
    Deck("Control Warrior [meta]", "Warrior", "control", [
        ("Execute", 2), ("Shield Slam", 2), ("Whirlwind", 2),
        ("Fiery War Axe", 2), ("Slam", 2), ("Cruel Taskmaster", 1),
        ("Armorsmith", 2), ("Acolyte of Pain", 2), ("Shield Block", 2),
        ("Death's Bite", 2), ("Brawl", 1), ("Sludge Belcher", 2),
        ("Shieldmaiden", 2), ("Big Game Hunter", 1),
        ("Sylvanas Windrunner", 1), ("Ragnaros the Firelord", 1),
        ("Grommash Hellscream", 1), ("Alexstrasza", 1), ("Dr. Boom", 1)]),
    Deck("Midrange Paladin [meta]", "Paladin", "midrange", [
        ("Zombie Chow", 2), ("Shielded Minibot", 2), ("Knife Juggler", 2),
        ("Muster for Battle", 2), ("Aldor Peacekeeper", 2),
        ("Truesilver Champion", 2), ("Consecration", 2),
        ("Blessing of Kings", 1), ("Piloted Shredder", 2),
        ("Quartermaster", 2), ("Sludge Belcher", 2),
        ("Antique Healbot", 2), ("Big Game Hunter", 1),
        ("Sylvanas Windrunner", 1), ("Lay on Hands", 1),
        ("Tirion Fordring", 1), ("Dr. Boom", 1), ("Equality", 2)]),
    Deck("Control Priest [meta]", "Priest", "control", [
        ("Circle of Healing", 2), ("Northshire Cleric", 2),
        ("Power Word: Shield", 2), ("Shadow Word: Pain", 2),
        ("Shadow Word: Death", 2), ("Wild Pyromancer", 2),
        ("Injured Blademaster", 2), ("Auchenai Soulpriest", 2),
        ("Holy Nova", 2), ("Holy Fire", 1), ("Mind Control", 1),
        ("Zombie Chow", 2), ("Sludge Belcher", 2), ("Cairne Bloodhoof", 1),
        ("Sylvanas Windrunner", 1), ("Ragnaros the Firelord", 1),
        ("Dr. Boom", 1), ("Earthen Ring Farseer", 2)]),
    Deck("Miracle Rogue [meta]", "Rogue", "midrange", [
        ("Backstab", 2), ("Preparation", 2), ("Deadly Poison", 2),
        ("Cold Blood", 2), ("Eviscerate", 2), ("Sap", 2), ("Shiv", 2),
        ("Fan of Knives", 2), ("SI:7 Agent", 2), ("Edwin VanCleef", 1),
        ("Gadgetzan Auctioneer", 2), ("Leeroy Jenkins", 1),
        ("Azure Drake", 2), ("Violet Teacher", 2),
        ("Bloodmage Thalnos", 1), ("Blade Flurry", 1),
        ("Loot Hoarder", 2)]),
    Deck("Midrange Shaman [meta]", "Shaman", "midrange", [
        ("Earth Shock", 2), ("Lightning Bolt", 2), ("Rockbiter Weapon", 2),
        ("Flametongue Totem", 2), ("Feral Spirit", 2), ("Hex", 2),
        ("Lava Burst", 1), ("Lightning Storm", 2), ("Unbound Elemental", 2),
        ("Mana Tide Totem", 2), ("Fire Elemental", 2), ("Azure Drake", 2),
        ("Al'Akir the Windlord", 1), ("Doomhammer", 1),
        ("Earth Elemental", 2), ("Sludge Belcher", 2), ("Dr. Boom", 1)]),
    Deck("Zoo Warlock [meta]", "Warlock", "aggro", [
        ("Flame Imp", 2), ("Voidwalker", 2), ("Argent Squire", 2),
        ("Abusive Sergeant", 2), ("Leper Gnome", 2), ("Knife Juggler", 2),
        ("Haunted Creeper", 2), ("Dire Wolf Alpha", 2),
        ("Nerubian Egg", 2), ("Shattered Sun Cleric", 2),
        ("Harvest Golem", 2), ("Dark Iron Dwarf", 2),
        ("Defender of Argus", 2), ("Doomguard", 2), ("Soulfire", 1),
        ("Sea Giant", 1)]),
    Deck("Combo Druid [meta]", "Druid", "midrange", [
        ("Innervate", 2), ("Wild Growth", 2), ("Wrath", 2), ("Swipe", 2),
        ("Savage Roar", 2), ("Keeper of the Grove", 2),
        ("Druid of the Claw", 2), ("Piloted Shredder", 2),
        ("Azure Drake", 2), ("Force of Nature", 2), ("Ancient of Lore", 2),
        ("Ancient of War", 1), ("Big Game Hunter", 1),
        ("Sylvanas Windrunner", 1), ("Cairne Bloodhoof", 1),
        ("Dr. Boom", 1), ("Nourish", 1), ("Loot Hoarder", 2)]),
    Deck("Tempo Demon Hunter [meta]", "Demon Hunter", "aggro", [
        ("Twin Slice", 2), ("Battlefiend", 2), ("Chaos Strike", 2),
        ("Umberwing", 2), ("Satyr Overseer", 2), ("Eye Beam", 2),
        ("Aldrachi Warblades", 2), ("Coordinated Strike", 2),
        ("Glaivebound Adept", 2), ("Skull of Gul'dan", 2),
        ("Priestess of Fury", 2), ("Chaos Nova", 1), ("Argent Squire", 2),
        ("Leper Gnome", 2), ("Wolfrider", 2), ("Leeroy Jenkins", 1)]),
]

CANDIDATES = [
    Deck("Freeze Mage [mine]", "Mage", "control", [
        ("Frostbolt", 2), ("Arcane Missiles", 2), ("Mirror Image", 2),
        ("Loot Hoarder", 2), ("Mad Scientist", 2), ("Ice Barrier", 1),
        ("Ice Block", 2), ("Frost Nova", 2), ("Arcane Intellect", 2),
        ("Counterspell", 1), ("Fireball", 2), ("Blizzard", 2),
        ("Antique Healbot", 2), ("Azure Drake", 2),
        ("Bloodmage Thalnos", 1), ("Alexstrasza", 1), ("Pyroblast", 1),
        ("Archmage Antonidas", 1)]),
    Deck("Mech-Tempo Mage [mine]", "Mage", "midrange", [
        ("Mana Wyrm", 2), ("Arcane Missiles", 2), ("Frostbolt", 2),
        ("Sorcerer's Apprentice", 2), ("Knife Juggler", 2),
        ("Mad Scientist", 2), ("Mirror Entity", 2),
        ("Arcane Intellect", 2), ("Fireball", 2), ("Water Elemental", 2),
        ("Azure Drake", 2), ("Argent Commander", 2), ("Flamestrike", 2),
        ("Piloted Shredder", 2), ("Archmage Antonidas", 1),
        ("Dr. Boom", 1)]),
    Deck("Face Hunter [mine]", "Hunter", "aggro", [
        ("Leper Gnome", 2), ("Webspinner", 2), ("Argent Squire", 2),
        ("Abusive Sergeant", 2), ("Knife Juggler", 2),
        ("Haunted Creeper", 2), ("Explosive Trap", 2),
        ("Animal Companion", 2), ("Kill Command", 2),
        ("Eaglehorn Bow", 2), ("Unleash the Hounds", 2), ("Wolfrider", 2),
        ("Mad Scientist", 2), ("Argent Commander", 2),
        ("Leeroy Jenkins", 1), ("Dire Wolf Alpha", 1)]),
    Deck("Hybrid Hunter [mine]", "Hunter", "midrange", [
        ("Webspinner", 2), ("Haunted Creeper", 2), ("Knife Juggler", 2),
        ("Mad Scientist", 2), ("Freezing Trap", 2),
        ("Animal Companion", 2), ("Kill Command", 2), ("Eaglehorn Bow", 2),
        ("Houndmaster", 2), ("Piloted Shredder", 2),
        ("Savannah Highmane", 2), ("Hunter's Mark", 2),
        ("Unleash the Hounds", 2), ("Sludge Belcher", 1),
        ("Dire Wolf Alpha", 2), ("Dr. Boom", 1)]),
    Deck("Tempo Warrior [mine]", "Warrior", "midrange", [
        ("Fiery War Axe", 2), ("Slam", 2), ("Cruel Taskmaster", 2),
        ("Armorsmith", 1), ("Frothing Berserker", 2),
        ("Acolyte of Pain", 2), ("Kor'kron Elite", 2), ("Execute", 2),
        ("Death's Bite", 2), ("Arcanite Reaper", 2),
        ("Piloted Shredder", 2), ("Chillwind Yeti", 2),
        ("Azure Drake", 2), ("Argent Commander", 2), ("Dr. Boom", 1),
        ("Grommash Hellscream", 1), ("Whirlwind", 1)]),
    Deck("Fatigue Warrior [mine]", "Warrior", "control", [
        ("Execute", 2), ("Shield Slam", 2), ("Whirlwind", 2),
        ("Fiery War Axe", 2), ("Armorsmith", 2), ("Slam", 2),
        ("Shield Block", 2), ("Acolyte of Pain", 2), ("Death's Bite", 2),
        ("Brawl", 2), ("Sludge Belcher", 2), ("Shieldmaiden", 2),
        ("Sen'jin Shieldmasta", 2), ("Big Game Hunter", 1),
        ("Sylvanas Windrunner", 1), ("Alexstrasza", 1),
        ("Ragnaros the Firelord", 1)]),
    Deck("Aggro Paladin [mine]", "Paladin", "aggro", [
        ("Argent Squire", 2), ("Leper Gnome", 2), ("Abusive Sergeant", 2),
        ("Shielded Minibot", 2), ("Knife Juggler", 2),
        ("Argent Protector", 2), ("Blessing of Might", 2),
        ("Muster for Battle", 2),
        ("Blessing of Kings", 2), ("Truesilver Champion", 2),
        ("Wolfrider", 2), ("Consecration", 2), ("Argent Commander", 2),
        ("Leeroy Jenkins", 1), ("Dr. Boom", 1), ("Dire Wolf Alpha", 2)]),
    Deck("Control Paladin [mine]", "Paladin", "control", [
        ("Zombie Chow", 2), ("Equality", 2), ("Wild Pyromancer", 2),
        ("Aldor Peacekeeper", 2), ("Truesilver Champion", 2),
        ("Consecration", 2), ("Muster for Battle", 2),
        ("Sludge Belcher", 2), ("Antique Healbot", 2),
        ("Quartermaster", 2), ("Stampeding Kodo", 2), ("Lay on Hands", 2),
        ("Big Game Hunter", 1), ("Sylvanas Windrunner", 1),
        ("Cairne Bloodhoof", 1), ("Tirion Fordring", 1),
        ("Ragnaros the Firelord", 1), ("Dr. Boom", 1)]),
    Deck("Injury Priest [mine]", "Priest", "control", [
        ("Circle of Healing", 2), ("Northshire Cleric", 2),
        ("Power Word: Shield", 2), ("Zombie Chow", 2),
        ("Wild Pyromancer", 2), ("Shadow Word: Pain", 2),
        ("Injured Blademaster", 2), ("Auchenai Soulpriest", 2),
        ("Shadow Word: Death", 2), ("Holy Nova", 2),
        ("Azure Drake", 2), ("Sludge Belcher", 2), ("Holy Fire", 2),
        ("Cairne Bloodhoof", 1), ("Ragnaros the Firelord", 1),
        ("Mind Control", 1), ("Dr. Boom", 1)]),
    Deck("Dragon-less Tempo Priest [mine]", "Priest", "midrange", [
        ("Northshire Cleric", 2), ("Power Word: Shield", 2),
        ("Zombie Chow", 2),
        ("Shadow Word: Pain", 2), ("Injured Blademaster", 2),
        ("Earthen Ring Farseer", 2), ("Chillwind Yeti", 2),
        ("Auchenai Soulpriest", 1), ("Circle of Healing", 1),
        ("Holy Nova", 2), ("Azure Drake", 2), ("Sludge Belcher", 2),
        ("Shadow Word: Death", 2), ("Boulderfist Ogre", 2),
        ("Cairne Bloodhoof", 1), ("Sylvanas Windrunner", 1),
        ("Dr. Boom", 1), ("Stormwind Champion", 1)]),
    Deck("Tempo Rogue [mine]", "Rogue", "midrange", [
        ("Backstab", 2), ("Deadly Poison", 2), ("SI:7 Agent", 2),
        ("Eviscerate", 2), ("Sap", 2), ("Edwin VanCleef", 1),
        ("Knife Juggler", 2), ("Loot Hoarder", 2), ("Fan of Knives", 2),
        ("Piloted Shredder", 2), ("Azure Drake", 2),
        ("Argent Commander", 2), ("Assassinate", 1),
        ("Sludge Belcher", 2), ("Leeroy Jenkins", 1), ("Dr. Boom", 1),
        ("Blade Flurry", 2)]),
    Deck("Miracle Teacher Rogue [mine]", "Rogue", "midrange", [
        ("Backstab", 2), ("Preparation", 2), ("Deadly Poison", 2),
        ("Cold Blood", 2), ("Eviscerate", 2), ("Sap", 2),
        ("Bloodmage Thalnos", 1), ("Shiv", 2), ("Fan of Knives", 2),
        ("SI:7 Agent", 2), ("Edwin VanCleef", 1), ("Violet Teacher", 2),
        ("Gadgetzan Auctioneer", 2), ("Azure Drake", 2),
        ("Leeroy Jenkins", 1), ("Assassinate", 1), ("Blade Flurry", 2)]),
    Deck("Aggro Shaman [mine]", "Shaman", "aggro", [
        ("Rockbiter Weapon", 2), ("Lightning Bolt", 2),
        ("Argent Squire", 2), ("Leper Gnome", 2), ("Abusive Sergeant", 2),
        ("Flametongue Totem", 2), ("Knife Juggler", 2),
        ("Feral Spirit", 2), ("Lava Burst", 2), ("Unbound Elemental", 2),
        ("Wolfrider", 2), ("Doomhammer", 2), ("Argent Commander", 2),
        ("Bloodlust", 1), ("Al'Akir the Windlord", 1),
        ("Leeroy Jenkins", 1), ("Dire Wolf Alpha", 1)]),
    Deck("Control Shaman [mine]", "Shaman", "control", [
        ("Earth Shock", 2), ("Lightning Bolt", 2), ("Rockbiter Weapon", 1),
        ("Flametongue Totem", 2), ("Feral Spirit", 2), ("Hex", 2),
        ("Lightning Storm", 2), ("Mana Tide Totem", 2),
        ("Unbound Elemental", 2), ("Azure Drake", 2),
        ("Fire Elemental", 2), ("Earth Elemental", 2),
        ("Sludge Belcher", 2), ("Antique Healbot", 2),
        ("Al'Akir the Windlord", 1), ("Ragnaros the Firelord", 1),
        ("Dr. Boom", 1)]),
    Deck("Handlock [mine]", "Warlock", "control", [
        ("Mortal Coil", 2), ("Darkbomb", 2), ("Ancient Watcher", 2),
        ("Sunfury Protector", 2), ("Ironbeak Owl", 2), ("Hellfire", 2),
        ("Shadow Bolt", 1), ("Twilight Drake", 2), ("Defender of Argus", 2),
        ("Big Game Hunter", 1), ("Doomguard", 1), ("Siphon Soul", 2),
        ("Sludge Belcher", 2), ("Antique Healbot", 2),
        ("Mountain Giant", 2), ("Dr. Boom", 1),
        ("Sylvanas Windrunner", 1), ("Alexstrasza", 1)]),
    Deck("Demon Zoo [mine]", "Warlock", "aggro", [
        ("Flame Imp", 2), ("Voidwalker", 2), ("Argent Squire", 2),
        ("Abusive Sergeant", 2), ("Soulfire", 2), ("Knife Juggler", 2),
        ("Haunted Creeper", 2), ("Nerubian Egg", 2), ("Dire Wolf Alpha", 2),
        ("Darkbomb", 2), ("Shattered Sun Cleric", 2),
        ("Defender of Argus", 2), ("Dark Iron Dwarf", 2),
        ("Doomguard", 2), ("Sea Giant", 2)]),
    Deck("Token Druid [mine]", "Druid", "midrange", [
        ("Innervate", 2), ("Wild Growth", 2), ("Wrath", 2),
        ("Haunted Creeper", 2), ("Violet Teacher", 2), ("Swipe", 2),
        ("Savage Roar", 2), ("Keeper of the Grove", 2),
        ("Druid of the Claw", 2), ("Force of Nature", 2),
        ("Azure Drake", 2), ("Ancient of Lore", 2), ("Ancient of War", 2),
        ("Dr. Boom", 1), ("Cairne Bloodhoof", 1), ("Sea Giant", 2)]),
    Deck("Ramp Druid [mine]", "Druid", "control", [
        ("Innervate", 2), ("Wild Growth", 2), ("Wrath", 2),
        ("Nourish", 2), ("Swipe", 2), ("Keeper of the Grove", 2),
        ("Druid of the Claw", 2), ("Sludge Belcher", 2),
        ("Ancient of Lore", 2), ("Ancient of War", 2),
        ("Big Game Hunter", 1), ("Sylvanas Windrunner", 1),
        ("Cairne Bloodhoof", 1), ("Ragnaros the Firelord", 1),
        ("Dr. Boom", 1), ("Boulderfist Ogre", 2), ("Chillwind Yeti", 2),
        ("Faceless Manipulator", 1)]),
    Deck("Big Demon Hunter [mine]", "Demon Hunter", "midrange", [
        ("Twin Slice", 2), ("Battlefiend", 2), ("Chaos Strike", 2),
        ("Umberwing", 2), ("Aldrachi Warblades", 2), ("Eye Beam", 2),
        ("Coordinated Strike", 2), ("Satyr Overseer", 2),
        ("Chaos Nova", 2), ("Glaivebound Adept", 2),
        ("Skull of Gul'dan", 2), ("Priestess of Fury", 2),
        ("Sludge Belcher", 2), ("Azure Drake", 2),
        ("Boulderfist Ogre", 2)]),
    Deck("Face Demon Hunter [mine]", "Demon Hunter", "aggro", [
        ("Twin Slice", 2), ("Battlefiend", 2), ("Leper Gnome", 2),
        ("Argent Squire", 2), ("Abusive Sergeant", 2),
        ("Chaos Strike", 2), ("Umberwing", 2), ("Satyr Overseer", 2),
        ("Eye Beam", 2), ("Knife Juggler", 2), ("Wolfrider", 2),
        ("Coordinated Strike", 2), ("Glaivebound Adept", 2),
        ("Argent Commander", 2), ("Leeroy Jenkins", 1),
        ("Skull of Gul'dan", 1)]),
]

# per-class candidate pool for optimizer swaps
POOLS = {
    "Mage": ["Mana Wyrm", "Sorcerer's Apprentice", "Water Elemental",
             "Polymorph", "Flamestrike", "Pyroblast", "Chillwind Yeti",
             "Argent Commander", "Piloted Shredder", "Dr. Boom",
             "Sludge Belcher", "Azure Drake", "Knife Juggler",
             "Loot Hoarder", "Mirror Image", "Ice Barrier"],
    "Hunter": ["Leper Gnome", "Argent Squire", "Wolfrider", "Houndmaster",
               "Savannah Highmane", "Piloted Shredder", "Hunter's Mark",
               "Freezing Trap", "Snake Trap", "Dr. Boom", "Sludge Belcher",
               "Dire Wolf Alpha", "Argent Commander", "Abusive Sergeant"],
    "Warrior": ["Whirlwind", "Cruel Taskmaster", "Armorsmith",
                "Frothing Berserker", "Kor'kron Elite", "Arcanite Reaper",
                "Brawl", "Shieldmaiden", "Grommash Hellscream",
                "Sen'jin Shieldmasta", "Piloted Shredder", "Azure Drake",
                "Chillwind Yeti", "Dr. Boom", "Big Game Hunter",
                "Sylvanas Windrunner", "Shield Slam"],
    "Paladin": ["Zombie Chow", "Shielded Minibot", "Argent Protector",
                "Blessing of Might", "Blessing of Kings", "Equality",
                "Aldor Peacekeeper", "Hammer of Wrath", "Quartermaster",
                "Antique Healbot", "Lay on Hands", "Tirion Fordring",
                "Piloted Shredder", "Knife Juggler", "Wild Pyromancer",
                "Stampeding Kodo"],
    "Priest": ["Circle of Healing", "Zombie Chow", "Wild Pyromancer",
               "Earthen Ring Farseer", "Injured Blademaster",
               "Auchenai Soulpriest", "Holy Fire", "Mind Control",
               "Chillwind Yeti", "Boulderfist Ogre", "Stormwind Champion",
               "Sylvanas Windrunner", "Azure Drake", "Sludge Belcher",
               "Power Word: Shield"],
    "Rogue": ["Preparation", "Cold Blood", "Shiv", "Blade Flurry",
              "Violet Teacher", "Gadgetzan Auctioneer", "Assassinate",
              "Piloted Shredder", "Argent Commander", "Sludge Belcher",
              "Dr. Boom", "Bloodmage Thalnos", "Knife Juggler",
              "Loot Hoarder", "Azure Drake", "Edwin VanCleef"],
    "Shaman": ["Earth Shock", "Rockbiter Weapon", "Lava Burst",
               "Lightning Storm", "Hex", "Mana Tide Totem", "Doomhammer",
               "Fire Elemental", "Earth Elemental", "Bloodlust",
               "Al'Akir the Windlord", "Sludge Belcher", "Azure Drake",
               "Argent Commander", "Dr. Boom", "Antique Healbot"],
    "Warlock": ["Flame Imp", "Voidwalker", "Soulfire", "Mortal Coil",
                "Darkbomb", "Shadow Bolt", "Hellfire", "Siphon Soul",
                "Twilight Drake", "Mountain Giant", "Sea Giant",
                "Doomguard", "Argent Squire", "Leper Gnome",
                "Nerubian Egg", "Harvest Golem", "Dark Iron Dwarf"],
    "Druid": ["Innervate", "Wild Growth", "Wrath", "Nourish", "Swipe",
              "Haunted Creeper", "Violet Teacher", "Sea Giant",
              "Piloted Shredder", "Azure Drake", "Big Game Hunter",
              "Cairne Bloodhoof", "Sylvanas Windrunner", "Dr. Boom",
              "Boulderfist Ogre", "Faceless Manipulator",
              "Ancient of War"],
    "Demon Hunter": ["Twin Slice", "Chaos Strike", "Umberwing",
                     "Aldrachi Warblades", "Eye Beam",
                     "Coordinated Strike", "Chaos Nova",
                     "Glaivebound Adept", "Skull of Gul'dan",
                     "Priestess of Fury", "Leper Gnome", "Argent Squire",
                     "Wolfrider", "Knife Juggler", "Argent Commander",
                     "Leeroy Jenkins", "Satyr Overseer"],
}
