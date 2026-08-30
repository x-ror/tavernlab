# Local commands.
#
# `make serve` is the whole product: one process, one page. Watching the
# game log, recording what you played and reading the record are all on
# that page now, so nothing here has to be typed while you play.
#
# What is left below the line is measurement -- the runs that check a claim
# the README makes. Those are deliberate, slow, and belong in a terminal.

.DEFAULT_GOAL := help

MANIFEST := tavernlab-sim/Cargo.toml
PROFILE  ?= debug
CARGOFLAGS :=
ifeq ($(PROFILE),release)
CARGOFLAGS := --release
endif
BIN  := tavernlab-sim/target/$(if $(filter release,$(PROFILE)),release,debug)/tavernsim
DATA := $(if $(TAVERNLAB_HOME),$(TAVERNLAB_HOME),$(HOME)/.local/share/TavernLab)

.NOTPARALLEL:

.PHONY: help build serve web history bench callgrind policy weights mulligan tiers

help:
	@echo "make serve          зібрати й відкрити застосунок (усе там: колода, гра, історія)"
	@echo "make web            перезібрати інтерфейс після зміни у web/"
	@echo "make build          лише зібрати (PROFILE=release для оптимізованого)"
	@echo "make history        показати записані бої в терміналі"
	@echo
	@echo "перевірки тверджень README:"
	@echo "make bench          пропускна здатність (див. tools/ab-bench.sh для порівнянь)"
	@echo "make callgrind      підрахунок інструкцій (детермінований A/B)"
	@echo "make policy         скільки віддає жадібний агент проти пошуку"
	@echo "make weights        чи ті числа стоять в оцінці позиції"
	@echo "make mulligan       чи порада мулігану залежить від політики"
	@echo "make tiers          чи їде тір-лист, коли політика міняється"
	@echo
	@echo "дані лежать у $(DATA), не в репозиторії"

build:
	cargo build --manifest-path $(MANIFEST) $(CARGOFLAGS)

# The interface is a build artefact, not a checked-in one, so a fresh clone
# needs this once. `serve` says so itself if it is missing.
web:
	cd web && npm install && npm run build

serve: build
	$(BIN) serve

# The same rows the History tab shows, for a terminal that is already open.
history: build
	$(BIN) history

# ------------------------------------------------------------ measurements

# Throughput of this checkout. To compare two builds, use tools/ab-bench.sh
# instead -- and read its header first: a naive best-of-five comparison on a
# shared host reports a 2% regression for a binary against a copy of itself.
bench: build
	$(BIN) bench

# How much the greedy policy gives up against a within-turn search. The A/A
# control in the first column must read 50.0%; anything else means the seat
# swap is broken and the columns beside it are not policy differences.
# Arguments are seeds per deck, node budget, depth, determinizations.
policy: build
	$(BIN) policy 200 4000 4 1

# One weight of the evaluation at a time, against the value it has. The
# control in the first row must read 50.0%: the same weights on both sides
# cannot differ, and anything else means the harness is measuring itself.
weights: build
	$(BIN) weights 400 4000

# Whether the mulligan advice depends on how well the agent plays -- and on
# how much of it is a coin toss either way. The first column is the control:
# the same policy on different seeds, which is the floor any policy
# difference has to clear.
mulligan: build
	$(BIN) mulligan 1500 data/gauntlet_standard.json

# Whether the tier list is a statement about decks or about the policy: the
# same table built twice, once by each. Slow -- the search side is ~200x the
# greedy one -- so this is a deliberate run, not part of `make test`.
tiers: build
	$(BIN) tiers 400 data/gauntlet_standard.json

# Instruction count instead of seconds: deterministic, so a 0.2% difference
# is a real one and needs no control run. About ten seconds under valgrind.
callgrind: build
	valgrind --tool=callgrind --callgrind-out-file=/tmp/tavernsim.cg \
	    $(BIN) bench 2000 1
	callgrind_annotate /tmp/tavernsim.cg | head -40
