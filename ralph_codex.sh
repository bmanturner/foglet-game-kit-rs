#!/usr/bin/env bash
# ralph_codex.sh — Ralph loop runner for Codex CLI (gpt-5.3-codex-spark @ high effort).
#
# Usage:
#   ./ralph_codex.sh                # 30 iters, 10M token cap
#   ./ralph_codex.sh 50             # 50 iters, 10M token cap
#   ./ralph_codex.sh 50 20000000    # 50 iters, 20M token cap
#
# Env overrides:
#   PROMPT_FILE       default: PROMPT.md
#   SPEC_FILE         default: SPEC_v4.md  (existence-checked at startup)
#   COMPLETION_TOKEN  default: <promise>COMPLETE</promise>
#   LOG_DIR           default: .ralph_codex
#   CODEX_MODEL       default: gpt-5.3-codex-spark
#   CODEX_EFFORT      default: high      (minimal | low | medium | high)
#   CODEX_FLAGS       extra flags appended to the codex invocation
#
# Each iteration:
#   1. Snapshots git HEAD (to detect whether the iter committed anything).
#   2. Pipes PROMPT.md to `codex exec --json` with the chosen model + effort,
#      capturing the JSONL event stream to .ralph_codex/iter-NNN.jsonl.
#   3. Parses the final `turn.completed` event for token usage.
#   4. Greps the final agent_message text for the completion sentinel.
#   5. Prints a one-line summary; updates cumulative tokens.
# Stops on: sentinel found, codex exit != 0, token cap hit, or max iters.
#
# Note: Codex does not emit per-turn USD cost in --json output (auth is typically
# subscription-based). We track total tokens (input + output) instead as a rough
# guardrail. Override COST tracking by editing the awk lines if you need $$.

set -euo pipefail

# ── Config ──────────────────────────────────────────────────────────────
MAX_ITERATIONS="${1:-30}"
MAX_TOKENS="${2:-1000000000}"
PROMPT_FILE="${PROMPT_FILE:-PROMPT.md}"
SPEC_FILE="${SPEC_FILE:-SPEC_v4.md}"
COMPLETION_TOKEN="${COMPLETION_TOKEN:-<promise>COMPLETE</promise>}"
LOG_DIR="${LOG_DIR:-.ralph_codex}"
CODEX_MODEL="${CODEX_MODEL:-gpt-5.3-codex-spark}"
CODEX_EFFORT="${CODEX_EFFORT:-high}"
CODEX_FLAGS="${CODEX_FLAGS:-}"

case "$CODEX_EFFORT" in
  minimal|low|medium|high) ;;
  *) echo "✗ CODEX_EFFORT='$CODEX_EFFORT' invalid (use: minimal|low|medium|high)"; exit 1 ;;
esac

# ── Pre-flight ──────────────────────────────────────────────────────────
need() { command -v "$1" >/dev/null || { echo "✗ $1 not found"; exit 1; }; }
need codex; need jq; need git; need awk
[[ -f "$PROMPT_FILE" ]] || { echo "✗ $PROMPT_FILE missing"; exit 1; }
[[ -f "$SPEC_FILE"   ]] || { echo "✗ $SPEC_FILE missing — pin the spec locally first"; exit 1; }
git rev-parse --git-dir >/dev/null 2>&1 || { echo "✗ not a git repo (run: git init)"; exit 1; }

mkdir -p "$LOG_DIR"
LOCK="$LOG_DIR/ralph.lock"
if [[ -f "$LOCK" ]]; then
  echo "✗ another ralph_codex appears to be running (lock: $LOCK)"
  echo "  if you're sure no other instance is alive: rm $LOCK"
  exit 1
fi
echo "$$" > "$LOCK"
trap 'rm -f "$LOCK"' EXIT INT TERM

# ── State ───────────────────────────────────────────────────────────────
TOTAL_TOKENS=0
RUN_LOG="$LOG_DIR/run-$(date +%Y%m%d-%H%M%S).log"

shopt -s nullglob
EXISTING_ITERS=( "$LOG_DIR"/iter-*.jsonl )
shopt -u nullglob
HIGHEST=0
if (( ${#EXISTING_ITERS[@]} > 0 )); then
  for f in "${EXISTING_ITERS[@]}"; do
    n="${f##*/iter-}"; n="${n%.jsonl}"
    n=$((10#$n))
    (( n > HIGHEST )) && HIGHEST=$n
  done
fi
START_N=$((HIGHEST + 1))
END_N=$((START_N + MAX_ITERATIONS - 1))

{
  echo "── ralph_codex.sh starting ──"
  echo "max_iterations=$MAX_ITERATIONS  max_tokens=$MAX_TOKENS"
  echo "model=$CODEX_MODEL  effort=$CODEX_EFFORT"
  echo "prompt=$PROMPT_FILE  spec=$SPEC_FILE"
  echo "log_dir=$LOG_DIR  iters=$START_N..$END_N (continuing from $HIGHEST)"
} | tee -a "$RUN_LOG"

# ── Loop ────────────────────────────────────────────────────────────────
for i in $(seq "$START_N" "$END_N"); do
  TS=$(date +%H:%M:%S)
  RUN_POS=$((i - START_N + 1))
  ITER_FILE=$(printf "%s/iter-%03d.jsonl" "$LOG_DIR" "$i")
  printf "\n── [%s] iter %03d  (%d/%d this run)  cum_tokens=%s ──\n" \
    "$TS" "$i" "$RUN_POS" "$MAX_ITERATIONS" "$TOTAL_TOKENS" | tee -a "$RUN_LOG"

  HEAD_BEFORE=$(git rev-parse HEAD 2>/dev/null || echo "none")

  # Run Codex non-interactively. --dangerously-bypass-approvals-and-sandbox
  # mirrors claude's --dangerously-skip-permissions for unattended ralph runs.
  set +e
  # --disable unified_exec: the experimental "unified exec" tool fails to
  #   spawn on some macOS setups with `Failed to create unified exec process:
  #   No such file or directory (os error 2)`. Forcing the classic shell-exec
  #   path is reliable.
  cat "$PROMPT_FILE" | codex exec \
    --json \
    -m "$CODEX_MODEL" \
    -c model_reasoning_effort="\"$CODEX_EFFORT\"" \
    --disable unified_exec \
    --dangerously-bypass-approvals-and-sandbox \
    --skip-git-repo-check \
    $CODEX_FLAGS \
    > "$ITER_FILE"
  CODEX_EXIT=$?
  set -e

  if [[ $CODEX_EXIT -ne 0 ]]; then
    echo "✗ codex exited $CODEX_EXIT — see $ITER_FILE" | tee -a "$RUN_LOG"
    exit 1
  fi

  # Final usage event
  USAGE=$(jq -c 'select(.type=="turn.completed") | .usage' "$ITER_FILE" | tail -1 || true)
  if [[ -z "$USAGE" || "$USAGE" == "null" ]]; then
    echo "✗ no turn.completed event found in $ITER_FILE" | tee -a "$RUN_LOG"
    exit 1
  fi

  ITER_IN=$(jq  -r '.input_tokens             // 0' <<<"$USAGE")
  ITER_OUT=$(jq -r '.output_tokens            // 0' <<<"$USAGE")
  ITER_REASON=$(jq -r '.reasoning_output_tokens // 0' <<<"$USAGE")
  ITER_CACHED=$(jq -r '.cached_input_tokens   // 0' <<<"$USAGE")
  ITER_TOTAL=$(( ITER_IN + ITER_OUT ))

  # Last agent message text (for sentinel match)
  ITER_TEXT=$(jq -r 'select(.type=="item.completed" and .item.type=="agent_message") | .item.text' "$ITER_FILE" | tail -1 || true)

  TOTAL_TOKENS=$(( TOTAL_TOKENS + ITER_TOTAL ))

  HEAD_AFTER=$(git rev-parse HEAD 2>/dev/null || echo "none")
  COMMITTED="no"; [[ "$HEAD_BEFORE" != "$HEAD_AFTER" ]] && COMMITTED="yes"

  printf "  in=%-7s out=%-6s reason=%-6s cached=%-7s committed=%s\n" \
    "$ITER_IN" "$ITER_OUT" "$ITER_REASON" "$ITER_CACHED" "$COMMITTED" \
    | tee -a "$RUN_LOG"

  if grep -qF "$COMPLETION_TOKEN" <<<"$ITER_TEXT"; then
    echo "✓ completion sentinel found"             | tee -a "$RUN_LOG"
    printf "── final cum tokens: %s ──\n" "$TOTAL_TOKENS" | tee -a "$RUN_LOG"
    exit 0
  fi

  if (( TOTAL_TOKENS >= MAX_TOKENS )); then
    echo "✗ token cap hit ($TOTAL_TOKENS >= $MAX_TOKENS)" | tee -a "$RUN_LOG"
    exit 2
  fi

  if [[ "$COMMITTED" == "no" ]]; then
    echo "  ⚠ no commit this iteration" | tee -a "$RUN_LOG"
  fi
done

echo "── max iterations reached without completion ──" | tee -a "$RUN_LOG"
printf "── final cum tokens: %s ──\n" "$TOTAL_TOKENS"   | tee -a "$RUN_LOG"
exit 3
