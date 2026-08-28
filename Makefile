# Local commands. Identity is not in this file: HS_ME / HS_LOGS / HS_DECK
# live in the environment, or in $(DATA)/watch.env, so a git pull cannot
# overwrite them.
#
# After updating the checkout: `make watch` rebuilds and restarts the
# recorder. History lives in the data home, not in the tree.

.DEFAULT_GOAL := help

MANIFEST := tavernlab-sim/Cargo.toml
PROFILE  ?= debug
CARGOFLAGS :=
ifeq ($(PROFILE),release)
CARGOFLAGS := --release
endif
BIN  := tavernlab-sim/target/$(if $(filter release,$(PROFILE)),release,debug)/tavernsim
DATA := $(if $(TAVERNLAB_HOME),$(TAVERNLAB_HOME),$(HOME)/.local/share/TavernLab)
PID  := $(DATA)/watch.pid
LOG  := $(DATA)/watch.log
ENV  := $(DATA)/watch.env

.PHONY: help build serve watch watch-start watch-stop watch-restart watch-status watch-log history

help:
	@echo "make watch          перезібрати й (пере)запустити запис ігор"
	@echo "make watch-stop     зупинити запис"
	@echo "make watch-status   чи працює"
	@echo "make watch-log      останні рядки лога демона"
	@echo "make history        показати записані бої"
	@echo "make serve          зібрати й відкрити інтерфейс"
	@echo "make build          лише зібрати (PROFILE=release для оптимізованого)"
	@echo
	@echo "бойовий тег і тека логів — у $(ENV), не в репозиторії:"
	@echo "  HS_ME='Ваш#12345'"
	@echo "  HS_LOGS='/шлях/до/Hearthstone/Logs'"

build:
	cargo build --manifest-path $(MANIFEST) $(CARGOFLAGS)

serve: build
	$(BIN) serve

# Rebuild and (re)start. The command to run after a git pull.
watch: watch-restart

watch-restart: watch-stop watch-start

watch-start: build
	@mkdir -p "$(DATA)"
	@if [ -f "$(PID)" ] && kill -0 $$(cat "$(PID)") 2>/dev/null; then \
		echo "вже працює, pid $$(cat "$(PID)"). make watch-restart — щоб перезібрати."; \
		exit 0; \
	fi; \
	if [ -f "$(ENV)" ]; then set -a; . "$(ENV)"; set +a; fi; \
	if [ -z "$$HS_ME" ]; then \
		echo "немає бойового тега. Запишіть його в $(ENV):"; \
		echo "  HS_ME='Ваш#12345'"; \
		echo "  HS_LOGS='/шлях/до/Hearthstone/Logs'"; \
		exit 2; \
	fi; \
	if [ -z "$$HS_LOGS" ]; then \
		echo "немає теки логів. Допишіть HS_LOGS у $(ENV)."; \
		exit 2; \
	fi; \
	nohup env HS_ME="$$HS_ME" HS_LOGS="$$HS_LOGS" HS_DECK="$$HS_DECK" \
		"$(CURDIR)/$(BIN)" watch --quiet >> "$(LOG)" 2>&1 & \
	echo $$! > "$(PID)"; \
	echo "записую ігри, pid $$(cat "$(PID)")"; \
	echo "лог: $(LOG)"

watch-stop:
	@if [ -f "$(PID)" ]; then \
		pid=$$(cat "$(PID)"); \
		if kill -0 $$pid 2>/dev/null; then \
			kill $$pid; \
			echo "зупинено pid $$pid"; \
		else \
			echo "процес уже не живий"; \
		fi; \
		rm -f "$(PID)"; \
	else \
		echo "не запущено"; \
	fi

watch-status:
	@if [ -f "$(PID)" ] && kill -0 $$(cat "$(PID)") 2>/dev/null; then \
		echo "працює, pid $$(cat "$(PID)")"; \
		echo "лог: $(LOG)"; \
	else \
		echo "не запущено"; \
	fi

watch-log:
	@if [ -f "$(LOG)" ]; then tail -n 80 "$(LOG)"; else echo "лога ще немає: $(LOG)"; fi

history: build
	$(BIN) history
