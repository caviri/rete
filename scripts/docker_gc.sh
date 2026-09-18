#!/usr/bin/env bash
# Garbage-collect the Docker leftovers a removed worktree leaves behind.
#
# WHY
#   compose.yaml pins the project name `rete`; a second checkout is told to
#   `export COMPOSE_PROJECT_NAME=my-worktree`, which gives it its own
#   `my-worktree_cargo-target` (and, before the registry was pinned to one shared
#   name, its own `my-worktree_cargo-registry`). Nothing deletes those when the
#   worktree goes: `git worktree remove` knows nothing about Docker. In September
#   2026 that was 70 volumes and 158 GB for worktrees that no longer existed.
#
# WHAT IT DOES (dry-run unless --apply)
#   1. Compose projects whose config files no longer exist on disk are torn down
#      (`docker compose -p NAME down -v --remove-orphans`): their exited
#      containers and their volumes go together.
#      Only rete-style projects count (a `compose.yaml` under a path containing
#      "rete"); another repo's dead project is left alone.
#   2. Volumes named `<project>_cargo-target`, `<project>_cargo-registry`,
#      `<project>_r-cargo-*` are removed when <project> is one of those dead
#      rete projects. A live project (config on disk, or the basename of a git
#      worktree of this repo, or --keep NAME) keeps its volumes; a volume whose
#      project has vanished from `docker compose ls` entirely is only reported,
#      because nothing says whose it was — pass --include NAME to remove it.
#      Volumes attached to a running container are never touched.
#   3. Named volumes this repo documents are always kept:
#      rete-cargo-registry, rete-r-cargo-registry, rete-cargo-bin.
#
#   Volumes that do not match the cargo naming pattern — other projects on the
#   same machine — are never considered.
#
# USAGE
#   scripts/docker_gc.sh            # report what would go, and how much
#   scripts/docker_gc.sh --apply    # do it
#   scripts/docker_gc.sh --keep scholarnq --apply   # extra project names to spare
#   scripts/docker_gc.sh --include audit236 --apply # remove an unattributed volume by project
#
# Run it after `git worktree remove`, or whenever `docker system df` looks wrong.
# DOCKER_GC_STDERR=/dev/stderr shows the errors of the compose listing parse.
set -uo pipefail
# Path conversion off for docker only; a global export breaks `git -C` on MSYS paths.
docker() { MSYS_NO_PATHCONV=1 command docker "$@"; }

APPLY=0
KEEP_EXTRA=()
INCLUDE_EXTRA=()
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) APPLY=1; shift ;;
    --keep)  KEEP_EXTRA+=("${2:?}"); shift 2 ;;
    --include) INCLUDE_EXTRA+=("${2:?}"); shift 2 ;;
    -h|--help) awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ALWAYS_KEEP="rete-cargo-registry rete-r-cargo-registry rete-cargo-bin"

say() { printf '%s\n' "$*"; }
would() { if [ "$APPLY" = 1 ]; then say "  $*"; else say "  (dry-run) $*"; fi; }

# --- live names ------------------------------------------------------------
live_projects=" rete "
for k in "${KEEP_EXTRA[@]:-}"; do [ -n "$k" ] && live_projects="$live_projects$k "; done
# every worktree of this repository, by directory basename (that is what people use as project name)
while IFS= read -r wt; do
  [ -n "$wt" ] && live_projects="$live_projects$(basename "$wt") "
done < <(git -C "$ROOT" worktree list --porcelain 2>/dev/null | awk '/^worktree /{print $2}')

# --- 1. dead compose projects ---------------------------------------------
say "== compose projects whose config files are gone"
dead_projects=()
other_projects=" "
# Existence is checked in Python: compose prints Windows paths on Docker Desktop
# and a bash `[ -e ]` on a backslash path is unreliable under MSYS.
while IFS=$'	' read -r name state; do
  state="${state%$'\r'}"; name="${name%$'\r'}"   # python on Windows writes CRLF to a pipe
  [ -z "$name" ] && continue
  case "$live_projects" in *" $name "*) continue ;; esac   # --keep / worktree basenames win
  case "$state" in
    alive)      live_projects="$live_projects$name " ;;
    dead-rete)  dead_projects+=("$name") ;;
    dead-other) other_projects="$other_projects$name " ;;   # another repo's project: never ours to touch
  esac
done < <(docker compose ls -a --format json 2>/dev/null | "$(command -v python || command -v python3)" -c '
import json, os, sys
for p in json.load(sys.stdin):
    files = [f.strip() for f in p.get("ConfigFiles", "").split(",") if f.strip()]
    alive = any(os.path.exists(f) for f in files)
    rete_style = any(os.path.basename(f) == "compose.yaml" and "rete" in f.lower() for f in files)
    print(p["Name"] + "	" + ("alive" if alive else ("dead-rete" if rete_style else "dead-other")))' 2> "${DOCKER_GC_STDERR:-/dev/null}")
for p in "${dead_projects[@]:-}"; do
  [ -z "$p" ] && continue
  would "docker compose -p $p down -v --remove-orphans"
  [ "$APPLY" = 1 ] && docker compose -p "$p" down -v --remove-orphans 2>&1 | sed 's/^/     /'
done
[ ${#dead_projects[@]} -eq 0 ] && say "  none"

# --- 2. orphan cargo volumes -----------------------------------------------
say "== cargo volumes (size per docker system df -v)"
declare -A size
while read -r n _ s _; do size["$n"]="$s"; done < <(docker system df -v 2>/dev/null | awk '/^VOLUME NAME/{p=1; next} p && NF==0{p=0} p')
total_rm=0; kept=0; removed=0
while IFS= read -r v; do
  [ -z "$v" ] && continue
  case " $ALWAYS_KEEP " in *" $v "*) say "  keep   $v  ${size[$v]:-?}  (documented shared volume)"; kept=$((kept+1)); continue ;; esac
  if [[ "$v" =~ ^(.+)_(r-)?cargo-(registry|target)$ ]]; then
    proj="${BASH_REMATCH[1]}"
    case "$live_projects" in *" $proj "*) say "  keep   $v  ${size[$v]:-?}  (project '$proj' is live)"; kept=$((kept+1)); continue ;; esac
    case "$other_projects" in *" $proj "*) say "  keep   $v  ${size[$v]:-?}  (project '$proj' belongs to another repo)"; kept=$((kept+1)); continue ;; esac
    known=0
    for d in "${dead_projects[@]:-}"; do [ "$d" = "$proj" ] && known=1; done
    case " ${INCLUDE_EXTRA[*]:-} " in *" $proj "*) known=1 ;; esac
    if [ "$known" = 0 ]; then
      say "  ?      $v  ${size[$v]:-?}  (no compose project left to attribute it; pass --include $proj to remove)"; continue
    fi
    would "docker volume rm $v  ${size[$v]:-?}"
    if [ "$APPLY" = 1 ]; then
      if docker volume rm "$v" >/dev/null 2>&1; then removed=$((removed+1)); else say "     FAILED (in use?): $(docker volume rm "$v" 2>&1 | tail -1)"; fi
    fi
  fi
done < <(docker volume ls --format '{{.Name}}' | grep -E 'cargo' | sort)

say "== summary: kept=$kept removed=$removed mode=$([ "$APPLY" = 1 ] && echo apply || echo dry-run)"
docker system df --format '{{.Type}}: {{.Size}} total, {{.Reclaimable}} reclaimable' | grep -i volumes
