//! Which set codes are legal in which format.
//!
//! Standard rotates every year, so this is a maintained list with a calendar
//! attached — no card database exposes "is this in the current Standard".
//! Legality is resolved here, at generation time, and baked into each card as
//! two bits; the engine never sees a set code on a hot path.

/// Sets legal in Standard for the current rotation.
pub const STANDARD: &[&str] = &[
    "CORE",
    "EMERALD_DREAM",
    "THE_LOST_CITY",
    "TIME_TRAVEL",
    "CATACLYSM",
    "ESCAPEFROM_VIOLET_HOLD",
    "EVENT",
    "PATH_OF_ARTHAS",
];

/// Sets legal in Wild: every real constructed set ever printed.
///
/// `VANILLA` is absent on purpose — the Classic format was retired, and every
/// card in it is a reprint of an `EXPERT1` card, so including it would
/// duplicate the whole Classic pool under a second code.
pub const WILD: &[&str] = &[
    "ALTERAC_VALLEY",
    "BATTLE_OF_THE_BANDS",
    "BLACK_TEMPLE",
    "BOOMSDAY",
    "BRM",
    "CATACLYSM",
    "CORE",
    "CORE_HIDDEN",
    "DALARAN",
    "DARKMOON_FAIRE",
    "DEMON_HUNTER_INITIATE",
    "DRAGONS",
    "EMERALD_DREAM",
    "ESCAPEFROM_VIOLET_HOLD",
    "EVENT",
    "EXPERT1",
    "GANGS",
    "GILNEAS",
    "GVG",
    "ICECROWN",
    "ISLAND_VACATION",
    "KARA",
    "LEGACY",
    "LOE",
    "LOOTAPALOOZA",
    "NAXX",
    "OG",
    "PATH_OF_ARTHAS",
    "RETURN_OF_THE_LICH_KING",
    "REVENDRETH",
    "SCHOLOMANCE",
    "SPACE",
    "STORMWIND",
    "TGT",
    "THE_BARRENS",
    "THE_LOST_CITY",
    "THE_SUNKEN_CITY",
    "TIME_TRAVEL",
    "TITANS",
    "TROLL",
    "ULDUM",
    "UNGORO",
    "WHIZBANGS_WORKSHOP",
    "WILD_WEST",
    "WONDERS",
    "YEAR_OF_THE_DRAGON",
];
