#!/usr/bin/env bash
# Live acceptance of the Docker demo network: outage replay, concurrent RFQs
# over one Maker pool or one corporate facility, selective MPC-node abort and
# a restart that spans the seven nodes, the gateway and DeFMI.
#
# Runs on the Docker host against an already running compose project.  Every
# observation is taken by `qomm-live-acceptance` (rust/qomm-demo/src/bin/
# qomm_live_acceptance.rs) from inside the demo network; this script only
# stops, kills and starts containers and records when it did so.  Nothing here
# signs a request or reads an outbox file.
#
# Run from the repository root against the project compose.yaml started
# (`docker compose -f demo-network/compose.yaml up -d`, project name
# `qomm-demo-network` unless -p was given):
#
#   QOMM_PROJECT=qomm-demo-network \
#   QOMM_ACCEPTANCE_OUT="$PWD/live-acceptance-out" \
#     demo-network/live_acceptance.sh all
#
# Then judge or aggregate from the recorded files alone:
#
#   docker run --rm -v "$PWD/live-acceptance-out:/out" qomm-demo-app:local \
#     qomm-live-acceptance judge --scenario outage-queue --dir /out
#   docker run --rm -v "$PWD/live-acceptance-out:/out" qomm-demo-app:local \
#     qomm-live-acceptance report --dir /out --out /out/acceptance.json --require outage-queue
#
# Environment: QOMM_PROJECT (compose project), QOMM_COMPOSE_FILE (defaults to
# the compose.yaml next to this script), QOMM_ACCEPTANCE_OUT (output
# directory), QOMM_APP_IMAGE (defaults to qomm-demo-app:local), QOMM_NETWORK
# (defaults to <project>_qomm-demo), QOMM_RFQ_TIMEOUT, QOMM_SETTLE_TIMEOUT.
#
# Scenarios: expired-seq7 restart-all outage-queue outage-dispatching
#            concurrent-facility concurrent-pool pool-sum pool-race pool-replay abort-1 abort-2 abort-3 all
set -euo pipefail

PROJECT=${QOMM_PROJECT:-qomm-demo-network}
COMPOSE_FILE=${QOMM_COMPOSE_FILE:-$(cd "$(dirname "$0")" && pwd)/compose.yaml}
OUT=${QOMM_ACCEPTANCE_OUT:-$PWD/live-acceptance-out}
APP_IMAGE=${QOMM_APP_IMAGE:-qomm-demo-app:local}
NETWORK=${QOMM_NETWORK:-${PROJECT}_qomm-demo}
RFQ_TIMEOUT=${QOMM_RFQ_TIMEOUT:-1500}
SETTLE_TIMEOUT=${QOMM_SETTLE_TIMEOUT:-2400}
NODES="mpc-0 mpc-1 mpc-2 mpc-3 mpc-4 mpc-5 mpc-6"

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
LOG="$OUT/acceptance.log"

log() {
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" | tee -a "$LOG"
}

mark() {
  # A machine-readable event line: scenario, event, unix time, detail.  The
  # detail is one line of plain text (control characters and quotes removed).
  local detail
  detail=$(printf '%s' "${3:-}" | tr -d '\n\r\t"\\')
  printf '{"scenario":"%s","event":"%s","at":%s,"detail":"%s"}\n' \
    "$1" "$2" "$(date +%s)" "$detail" >> "$OUT/events.jsonl"
  log "[$1] $2 $detail"
}

compose() {
  docker compose -p "$PROJECT" -f "$COMPOSE_FILE" "$@"
}

container() {
  printf '%s-%s-1' "$PROJECT" "$1"
}

acc() {
  docker run --rm --network "$NETWORK" --user "$(id -u):$(id -g)" \
    -v "$OUT:/out" "$APP_IMAGE" qomm-live-acceptance "$@"
}

acc_bg() {
  # Same as acc but detached; prints the container id.
  docker run -d --network "$NETWORK" --user "$(id -u):$(id -g)" \
    -v "$OUT:/out" "$APP_IMAGE" qomm-live-acceptance "$@"
}

snapshot() {
  acc snapshot --out "/out/$1.json" > /dev/null
  log "snapshot $1"
}

next_sequence() {
  # The taker outbox sequence the next accepted request will receive.
  acc snapshot --reconcile 0 | grep -m1 '"next_sequence"' | tr -dc '0-9'
}

wait_healthy() {
  local deadline=$(( $(date +%s) + ${WAIT_HEALTHY_SECS:-600} ))
  for svc in "$@"; do
    while true; do
      local status
      status=$(docker inspect -f '{{.State.Health.Status}}' "$(container "$svc")" 2>/dev/null || echo missing)
      [ "$status" = healthy ] && break
      if [ "$(date +%s)" -ge "$deadline" ]; then
        log "timeout waiting for $svc to be healthy (status $status)"
        return 1
      fi
      sleep 2
    done
  done
  log "healthy: $*"
}

stop_nodes() {
  for svc in "$@"; do docker stop -t 2 "$(container "$svc")" > /dev/null; done
  mark "$SCENARIO" stopped "$*"
}

start_nodes() {
  compose start "$@" > /dev/null 2>&1
  mark "$SCENARIO" started "$*"
  wait_healthy "$@"
}

pause_nodes() {
  # Paused containers keep their names in the network's DNS while every
  # connection to them times out: the committee is unreachable, and a
  # gateway restarted meanwhile still resolves its peers and comes up.
  for svc in "$@"; do docker pause "$(container "$svc")" > /dev/null; done
  mark "$SCENARIO" paused "$*"
}

unpause_nodes() {
  for svc in "$@"; do docker unpause "$(container "$svc")" > /dev/null 2>&1 || true; done
  mark "$SCENARIO" unpaused "$*"
  wait_healthy "$@"
}

collect_logs() {
  # Gateway and taker logs since the scenario began, for the record.
  local since=$1
  for svc in gateway taker; do
    docker logs --since "$since" "$(container "$svc")" > "$OUT/$SCENARIO.$svc.log" 2>&1 || true
  done
}

scenario_begin() {
  SCENARIO=$1
  SCENARIO_SINCE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  mark "$SCENARIO" begin
  snapshot "$SCENARIO.00-before"
}

scenario_end() {
  snapshot "$SCENARIO.99-after"
  collect_logs "$SCENARIO_SINCE"
  mark "$SCENARIO" end
}

# --- AC1: the signed request that expired in the queue ---------------------
expired_seq7() {
  scenario_begin expired-seq7
  local seq=${QOMM_EXPIRED_SEQUENCE:-7}
  acc wait --sequence "$seq" --until released --timeout-secs 5 --out "/out/$SCENARIO.01-entry.json" > /dev/null || true
  # Negative probe: the participant module refuses another body under the
  # same request id, and refuses a request whose expiry already passed.
  local id
  id=$(grep -m1 '"request_id"' "$OUT/$SCENARIO.01-entry.json" | sed 's/.*: "\([0-9a-f]*\)".*/\1/')
  docker run --rm --network "$NETWORK" "$APP_IMAGE" curl -s -o /dev/stdout -w '\n%{http_code}\n' \
    -X POST -H 'content-type: application/json' "http://taker:9200/v1/outbox/requests" \
    -d "{\"request_id\":\"$id\",\"signed_request\":\"AAAA\",\"expires_at\":4102444800}" \
    > "$OUT/$SCENARIO.02-reuse-probe.txt" 2>&1 || true
  docker run --rm --network "$NETWORK" "$APP_IMAGE" curl -s -o /dev/stdout -w '\n%{http_code}\n' \
    -X POST -H 'content-type: application/json' "http://taker:9200/v1/outbox/requests" \
    -d "{\"request_id\":\"$id\",\"signed_request\":\"AAAA\",\"expires_at\":1}" \
    > "$OUT/$SCENARIO.03-expired-probe.txt" 2>&1 || true
  scenario_end
}

# --- AC5: restart across the seven nodes, the gateway and DeFMI -------------
restart_all() {
  scenario_begin restart-all
  compose restart $NODES > /dev/null 2>&1
  mark "$SCENARIO" restarted "$NODES"
  compose restart defmi-network > /dev/null 2>&1
  mark "$SCENARIO" restarted defmi-network
  wait_healthy defmi-network $NODES
  compose restart gateway > /dev/null 2>&1
  mark "$SCENARIO" restarted gateway
  wait_healthy gateway
  snapshot "$SCENARIO.01-after-restart"
  local last=$(( $(next_sequence) - 1 ))
  acc rfq --asset 0 --direction 0 --qty 1 --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.02-rfq.json" > /dev/null || true
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.03-wait.json" > /dev/null || true
  scenario_end
}

# --- AC2 (a): accepted into the durable queue while the committee is down ---
outage_queue() {
  scenario_begin outage-queue
  local last=$(( $(next_sequence) - 1 ))
  stop_nodes $NODES
  # The signed RFQ must be accepted and stay `queued`; nothing can execute.
  acc rfq --asset 0 --direction 0 --qty 1 --timeout-secs 300 --out "/out/$SCENARIO.01-rfq-while-down.json" > /dev/null || true
  acc wait --after-sequence "$last" --until present --timeout-secs 10 --out "/out/$SCENARIO.02-queued.json" > /dev/null || true
  docker stop -t 2 "$(container gateway)" > /dev/null
  mark "$SCENARIO" stopped gateway
  snapshot "$SCENARIO.03-all-down"
  sleep 20
  snapshot "$SCENARIO.04-still-queued"
  start_nodes $NODES
  compose start gateway > /dev/null 2>&1
  mark "$SCENARIO" started gateway
  wait_healthy gateway
  # No browser, no new signature: the ticker replays the queued request.
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.05-replayed.json" > /dev/null || true
  scenario_end
}

# --- AC2 (b): gateway killed after the DeFMI reserve, before settlement -----
outage_dispatching() {
  scenario_begin outage-dispatching
  local last=$(( $(next_sequence) - 1 ))
  local rfq
  rfq=$(acc_bg rfq --asset 0 --direction 0 --qty 1 --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.01-rfq.json")
  acc wait --after-sequence "$last" --until hold-active --timeout-secs 900 --out "/out/$SCENARIO.02-hold-active.json" > /dev/null || true
  docker kill "$(container gateway)" > /dev/null
  mark "$SCENARIO" killed gateway
  docker wait "$rfq" > /dev/null 2>&1 || true
  docker logs "$rfq" > "$OUT/$SCENARIO.01-rfq.stdout" 2>&1 || true
  docker rm "$rfq" > /dev/null 2>&1 || true
  snapshot "$SCENARIO.03-gateway-down"
  compose start gateway > /dev/null 2>&1
  mark "$SCENARIO" started gateway
  wait_healthy gateway
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.04-replayed.json" > /dev/null || true
  scenario_end
}

# --- AC3 (facility): two sells whose sum exceeds the Taker inventory cap ----
concurrent_facility() {
  scenario_begin concurrent-facility
  local last=$(( $(next_sequence) - 1 ))
  stop_nodes $NODES
  acc rfq --asset 0 --direction 1 --qty "${QOMM_FACILITY_QTY:-200}" --timeout-secs 300 --out "/out/$SCENARIO.01-rfq-a.json" > /dev/null || true
  acc rfq --asset 0 --direction 1 --qty "${QOMM_FACILITY_QTY:-200}" --timeout-secs 300 --out "/out/$SCENARIO.02-rfq-b.json" > /dev/null || true
  snapshot "$SCENARIO.03-both-submitted"
  start_nodes $NODES
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.04-a-finalized.json" > /dev/null || true
  sleep 5
  snapshot "$SCENARIO.05-after-a"
  # Same request again once the first one is final: the cap is about
  # outstanding reservations, and DeFMI still holds only what exists.
  acc rfq --asset 0 --direction 1 --qty "${QOMM_FACILITY_QTY:-200}" --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.06-rfq-c.json" > /dev/null || true
  scenario_end
}

# --- AC3 (pool): two buys whose sum exceeds one Maker's standing pool -------
concurrent_pool() {
  scenario_begin concurrent-pool
  local maker=${QOMM_POOL_MAKER:-3}
  local maxqty=${QOMM_POOL_MAXQTY:-200}
  local qty=${QOMM_POOL_QTY:-150}
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$other-off.json" > /dev/null || true
    fi
  done
  acc policy --maker "$maker" --active 1 --maxqty "$maxqty" --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$maker-on.json" > /dev/null || true
  snapshot "$SCENARIO.02-policies"
  local last=$(( $(next_sequence) - 1 ))
  stop_nodes $NODES
  acc rfq --asset 0 --direction 0 --qty "$qty" --timeout-secs 300 --out "/out/$SCENARIO.03-rfq-a.json" > /dev/null || true
  acc rfq --asset 0 --direction 0 --qty "$qty" --timeout-secs 300 --out "/out/$SCENARIO.04-rfq-b.json" > /dev/null || true
  snapshot "$SCENARIO.05-both-queued"
  start_nodes $NODES
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.06-a-finalized.json" > /dev/null || true
  snapshot "$SCENARIO.07-after-a"
  acc wait --after-sequence $(( last + 1 )) --until "${QOMM_POOL_B_UNTIL:-finalized}" --timeout-secs "${QOMM_POOL_B_TIMEOUT:-$SETTLE_TIMEOUT}" --out "/out/$SCENARIO.08-b.json" > /dev/null || true
  snapshot "$SCENARIO.09-after-b"
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 1 --timeout-secs 300 --out "/out/$SCENARIO.10-policy-maker$other-restore.json" > /dev/null || true
    fi
  done
  scenario_end
}

# --- AC3 (pool sum): two buys whose sum exceeds one Maker's pool remainder --
# The Maker's inventory skew is pinned at its clamp so the fill does not
# rewrite the policy (a rewritten policy is a new pool at sequence 0); the
# second buy therefore meets the same pool with only the remainder left.
# The skew and depth coefficients are zero so the pinned skew does not push
# the ask above the Taker's signed limit (the first attempt of this scenario
# priced maker 3 out with invcoef 1, inv 120, slope 3 and filled nothing).
pool_sum() {
  scenario_begin pool-sum
  local maker=${QOMM_POOL_MAKER:-3}
  local maxqty=${QOMM_POOL_SUM_MAXQTY:-30}
  local qty=${QOMM_POOL_SUM_QTY:-20}
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$other-off.json" > /dev/null || true
    fi
  done
  acc policy --maker "$maker" --active 1 --asset 0 --maxqty "$maxqty" --inv 120 --invcoef 0 --slope 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$maker-on.json" > /dev/null || true
  snapshot "$SCENARIO.02-policies"
  local last=$(( $(next_sequence) - 1 ))
  local rfq
  rfq=$(acc_bg rfq --asset 0 --direction 0 --qty "$qty" --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.03-rfq-a.json")
  acc wait --after-sequence "$last" --until hold-active --timeout-secs 900 --out "/out/$SCENARIO.04-a-hold-active.json" > /dev/null || true
  # A second buy while the first is still reserved: the corporate boundary
  # admits one outstanding Taker reservation, so this must be refused.
  acc rfq --asset 0 --direction 0 --qty "$qty" --timeout-secs 120 --out "/out/$SCENARIO.05-rfq-b-concurrent.json" > /dev/null || true
  docker wait "$rfq" > /dev/null 2>&1 || true
  docker logs "$rfq" > "$OUT/$SCENARIO.03-rfq-a.stdout" 2>&1 || true
  docker rm "$rfq" > /dev/null 2>&1 || true
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.06-a-finalized.json" > /dev/null || true
  snapshot "$SCENARIO.07-after-a"
  # The same buy again once A is final: pool remainder maxqty - qty < qty.
  # The room clears the Taker's pre-trade reservation on its next tick after
  # the settlement; a refusal for that reason alone is retried a few times
  # and every attempt is kept.
  local attempt=1
  while :; do
    acc rfq --asset 0 --direction 0 --qty "$qty" --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.08-rfq-b.json" > /dev/null || true
    if grep -q "previous Taker reservation is still active" "$OUT/$SCENARIO.08-rfq-b.json" && [ "$attempt" -lt "${QOMM_POOL_B_ATTEMPTS:-6}" ]; then
      cp "$OUT/$SCENARIO.08-rfq-b.json" "$OUT/$SCENARIO.08-rfq-b-attempt$attempt.json"
      attempt=$(( attempt + 1 ))
      sleep 30
      continue
    fi
    break
  done
  acc wait --after-sequence $(( last + 1 )) --until "${QOMM_POOL_B_UNTIL:-present}" --timeout-secs "${QOMM_POOL_B_TIMEOUT:-60}" --out "/out/$SCENARIO.09-b.json" > /dev/null || true
  sleep "${QOMM_POOL_B_OBSERVE:-420}"
  acc wait --after-sequence $(( last + 1 )) --until present --timeout-secs 60 --out "/out/$SCENARIO.10-b-later.json" > /dev/null || true
  snapshot "$SCENARIO.11-after-b"
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 1 --timeout-secs 300 --out "/out/$SCENARIO.12-policy-maker$other-restore.json" > /dev/null || true
    fi
  done
  scenario_end
}

# --- AC3 (pool race): two signed buys durably admitted against one pool ------
# Two distinct signed, pre-authorized requests for the same Maker pool are
# both admitted into the corporate outbox while the committee is down, so
# neither the room's single-reservation rule nor its reserve mirror can
# decide between them: the gateway keeps one Taker reservation per process,
# so the second request is signed after a gateway restart, and the corporate
# module admits it because both fit the entity's cash facility.  When the
# committee returns the replay ticker executes both from their stored policy
# and reserve snapshots; the second exceeds the pool remainder the first
# left, and whatever stops it (the resident-state generation, the DvP
# remainder proof, or DeFMI's pool-note compare-and-swap) is recorded from
# the gateway's own log and the canonical state.
pool_race() {
  scenario_begin pool-race
  local maker=${QOMM_POOL_MAKER:-3}
  # A quantity that differs from `pool-sum` so the Maker registers a fresh
  # standing mandate and pool for this scenario (the same digest would keep
  # the pool the earlier fill drained).
  # A small pool and small quantities: two buys of QTY exceed the MAXQTY-unit
  # Maker pool, but each Taker cash reserve (QTY x limit) is tiny, so the
  # Taker's own canonical cash notes are not the binding resource --- the
  # Maker pool remainder is, and the second buy meets the DeFMI pool guard
  # (the resident-state generation, the DvP remainder range proof, or the
  # pool-note compare-and-swap) rather than a Taker-facility refusal.
  local maxqty=${QOMM_POOL_RACE_MAXQTY:-3}
  local qty=${QOMM_POOL_RACE_QTY:-2}
  # Sells by default: the Taker delivers inventory, of which its canonical
  # notes hold hundreds of units, while the Maker's cash pool covers MAXQTY
  # units; the long-running demo's Taker cash notes are nearly spent by the
  # earlier buys, so buys would be refused at the Taker's own facility.
  local direction=${QOMM_POOL_RACE_DIRECTION:-1}
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$other-off.json" > /dev/null || true
    fi
  done
  acc policy --maker "$maker" --active 1 --asset 0 --maxqty "$maxqty" --inv 120 --invcoef 0 --slope 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$maker-on.json" > /dev/null || true
  snapshot "$SCENARIO.02-policies"
  local last=$(( $(next_sequence) - 1 ))
  pause_nodes $NODES
  acc rfq --asset 0 --direction "$direction" --qty "$qty" --timeout-secs 300 --out "/out/$SCENARIO.03-rfq-a.json" > /dev/null || true
  acc wait --after-sequence "$last" --until present --timeout-secs 30 --out "/out/$SCENARIO.04-a-queued.json" > /dev/null || true
  # The room keeps one Taker reservation per process; restarting the gateway
  # (with the committee still unreachable) lets the second request be signed
  # and admitted through the same corporate path.  Everything durable
  # survives the restart.
  compose restart gateway > /dev/null 2>&1
  mark "$SCENARIO" restarted gateway
  if ! wait_healthy gateway; then
    mark "$SCENARIO" aborted "gateway did not become healthy while the committee was paused"
    unpause_nodes $NODES
    scenario_end
    return
  fi
  # The Maker seats, their manual pins and their policies live in gateway
  # memory: after the restart the bots would quote again with their own
  # policies and the second request would be signed against a different
  # market.  The same single-pool market is re-applied first; the identical
  # policy digest keeps the standing pool the first request was signed
  # against, so both envelopes name the same Maker authority set.
  pin_makers 1
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 0 --timeout-secs 300 --out "/out/$SCENARIO.04b-policy-maker$other-off-after-restart.json" > /dev/null || true
    fi
  done
  acc policy --maker "$maker" --active 1 --asset 0 --maxqty "$maxqty" --inv 120 --invcoef 0 --slope 0 --timeout-secs 300 --out "/out/$SCENARIO.04b-policy-maker$maker-on-after-restart.json" > /dev/null || true
  snapshot "$SCENARIO.04c-policies-after-restart"
  acc rfq --asset 0 --direction "$direction" --qty "$qty" --timeout-secs 300 --out "/out/$SCENARIO.05-rfq-b.json" > /dev/null || true
  acc wait --after-sequence $(( last + 1 )) --until present --timeout-secs 30 --out "/out/$SCENARIO.06-b-queued.json" > /dev/null || true
  snapshot "$SCENARIO.07-both-queued"
  local journal_before
  journal_before=$(docker exec "$(container gateway)" sh -c 'ls /home/qomm/defmi-journal 2>/dev/null | grep -c issueStandingPoolProductSettlement || true')
  unpause_nodes $NODES
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.08-a-finalized.json" > /dev/null || true
  snapshot "$SCENARIO.09-after-a"
  acc wait --after-sequence $(( last + 1 )) --until finalized --timeout-secs "${QOMM_POOL_RACE_B_TIMEOUT:-900}" --out "/out/$SCENARIO.10-b-finalized.json" > /dev/null || true
  snapshot "$SCENARIO.11-after-b"
  # The allocations the gateway issued for each settlement, verbatim from its
  # DeFMI journal (the ticker re-registers pools between rounds, so the node
  # bindings alone cannot say which pool each settlement drew from), and the
  # canonical state of the first request's pool once the second is final.
  local settled_journals
  settled_journals=$(docker exec "$(container gateway)" sh -c 'ls /home/qomm/defmi-journal | grep issueStandingPoolProductSettlement | tail -n +'"$(( journal_before + 1 ))")
  local index=0
  for entry in $settled_journals; do
    index=$(( index + 1 ))
    docker cp "$(container gateway):/home/qomm/defmi-journal/$entry" "$OUT/$SCENARIO.11b-journal-$index.json" || true
  done
  mark "$SCENARIO" journaled "$index settlement(s) after $journal_before"
  local pool_a
  pool_a=$(grep -m1 '"poolID"' "$OUT/$SCENARIO.11b-journal-1.json" 2>/dev/null | sed 's/.*"poolID": *"\([0-9a-f]*\)".*/\1/')
  if [ -n "$pool_a" ]; then
    acc pool --id "$pool_a" --out "/out/$SCENARIO.11c-pool-a.json" > /dev/null || true
  fi
  sleep "${QOMM_POOL_B_OBSERVE:-120}"
  acc wait --after-sequence $(( last + 1 )) --until present --timeout-secs 30 --out "/out/$SCENARIO.12-b-later.json" > /dev/null || true
  snapshot "$SCENARIO.13-after-b-later"
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 1 --timeout-secs 300 --out "/out/$SCENARIO.14-policy-maker$other-restore.json" > /dev/null || true
    fi
  done
  scenario_end
}

# --- AC3 (pool replay): the L1's own pool guard, exercised directly ---------
# An honest gateway never submits an allocation it knows exceeds the pool:
# its reserve mirror refuses the match before DeFMI (pool-sum, pool-race).
# The canonical guard behind it is observable only by presenting a valid
# allocation a second time.  One request settles normally; the allocation
# the gateway issued for it is taken verbatim from its DeFMI journal and
# re-presented to the L1 (the product settlement that carries the standing
# pool allocation), which must refuse it because the pool's current note is
# no longer the parent it names and the state root it expected has moved.  Nothing is bypassed and nothing is
# forged: the transition, its proofs and its committee signature are the
# accepted ones, applied once.
pool_replay() {
  scenario_begin pool-replay
  local maker=${QOMM_POOL_MAKER:-3}
  # A pool size no earlier scenario used on this network: re-applying a
  # policy whose digest an earlier fill drained keeps the drained reserve
  # under the old mandate and the request aborts before any hold.
  local maxqty=${QOMM_POOL_REPLAY_MAXQTY:-5}
  local qty=${QOMM_POOL_REPLAY_QTY:-2}
  local direction=${QOMM_POOL_REPLAY_DIRECTION:-1}
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$other-off.json" > /dev/null || true
    fi
  done
  acc policy --maker "$maker" --active 1 --asset 0 --maxqty "$maxqty" --inv 120 --invcoef 0 --slope 0 --timeout-secs 300 --out "/out/$SCENARIO.01-policy-maker$maker-on.json" > /dev/null || true
  snapshot "$SCENARIO.02-policies"
  local last=$(( $(next_sequence) - 1 ))
  local journal_before
  journal_before=$(docker exec "$(container gateway)" sh -c 'ls /home/qomm/defmi-journal 2>/dev/null | grep -c issueStandingPoolProductSettlement || true')
  acc rfq --asset 0 --direction "$direction" --qty "$qty" --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.03-rfq.json" > /dev/null || true
  # Only the record's own top-level field: the embedded before/after views
  # carry the previous round's abort code.
  if grep -q '^  "abort_code": "engine"' "$OUT/$SCENARIO.03-rfq.json"; then
    mark "$SCENARIO" aborted "the request was aborted by the engine before any hold; nothing to replay"
    snapshot "$SCENARIO.05-after-settle"
    scenario_end
    return
  fi
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.04-wait.json" > /dev/null || true
  snapshot "$SCENARIO.05-after-settle"
  local latest
  latest=$(docker exec "$(container gateway)" sh -c 'ls /home/qomm/defmi-journal | grep issueStandingPoolProductSettlement | tail -1')
  mark "$SCENARIO" journaled "$latest (allocations before: $journal_before)"
  docker cp "$(container gateway):/home/qomm/defmi-journal/$latest" "$OUT/$SCENARIO.06-journaled-allocation.json"
  local height
  height=$(grep -m1 '"ledger_height"' "$OUT/$SCENARIO.04-wait.json" | tr -dc '0-9')
  # (i) the identical bytes: the L1 deduplicates them by transaction id;
  # (ii) the same allocation under the live state root: a new transaction
  # that the pool-note compare-and-swap must refuse.
  acc defmi-rpc --file "/out/$SCENARIO.06-journaled-allocation.json" --original-height "${height:-0}" --out "/out/$SCENARIO.07-replay.json" > /dev/null || true
  acc defmi-rpc --file "/out/$SCENARIO.06-journaled-allocation.json" --before-root current --original-height "${height:-0}" --out "/out/$SCENARIO.07b-replay-fresh-root.json" > /dev/null || true
  # (iii) the same allocation, built against the live root, but with the
  # remainder inflated: the canonical VM's standing-pool allocation body
  # rejects it for taking more than the parent note holds, before the
  # approval or the root are checked.
  acc pool-guard-probe --file "/out/$SCENARIO.06-journaled-allocation.json" --out "/out/$SCENARIO.07c-pool-guard.json" > /dev/null || true
  # (iv) a conserving allocation, freshly re-approved by a genuine 3-of-7
  # development-committee quorum over the live root, but naming the pool
  # sequence/current-note the fill already advanced past: the canonical VM's
  # standing-pool sequence/current-note compare-and-swap must reject it, with
  # a corrupted-approval and a stale-root control to prove the approval and
  # root gates precede the compare-and-swap.
  acc pool-cas-probe --file "/out/$SCENARIO.06-journaled-allocation.json" --out "/out/$SCENARIO.07d-pool-cas.json" > /dev/null || true
  snapshot "$SCENARIO.08-after-replay"
  for other in 0 1 2 3; do
    if [ "$other" != "$maker" ]; then
      acc policy --maker "$other" --active 1 --timeout-secs 300 --out "/out/$SCENARIO.09-policy-maker$other-restore.json" > /dev/null || true
    fi
  done
  scenario_end
}

# --- AC4: stop nodes while the round is executing ---------------------------
abort_nodes() {
  local count=$1
  shift
  scenario_begin "abort-$count"
  local victims="$*"
  local last=$(( $(next_sequence) - 1 ))
  local rfq
  rfq=$(acc_bg rfq --asset 0 --direction 0 --qty 1 --timeout-secs "$RFQ_TIMEOUT" --out "/out/$SCENARIO.01-rfq.json")
  acc wait --after-sequence "$last" --until hold-active --timeout-secs 900 --out "/out/$SCENARIO.02-hold-active.json" > /dev/null || true
  sleep "${QOMM_ABORT_DELAY:-3}"
  stop_nodes $victims
  docker wait "$rfq" > /dev/null 2>&1 || true
  docker logs "$rfq" > "$OUT/$SCENARIO.01-rfq.stdout" 2>&1 || true
  docker rm "$rfq" > /dev/null 2>&1 || true
  snapshot "$SCENARIO.03-after-abort"
  sleep 15
  snapshot "$SCENARIO.04-still-reserved"
  start_nodes $victims
  acc wait --after-sequence "$last" --until finalized --timeout-secs "$SETTLE_TIMEOUT" --out "/out/$SCENARIO.05-replayed.json" > /dev/null || true
  scenario_end
}

run() {
  case "$1" in
    expired-seq7) expired_seq7 ;;
    restart-all) restart_all ;;
    outage-queue) outage_queue ;;
    outage-dispatching) outage_dispatching ;;
    concurrent-facility) concurrent_facility ;;
    concurrent-pool) concurrent_pool ;;
    pool-sum) pool_sum ;;
    pool-race) pool_race ;;
    pool-replay) pool_replay ;;
    abort-1) abort_nodes 1 mpc-6 ;;
    abort-2) abort_nodes 2 mpc-5 mpc-6 ;;
    abort-3) abort_nodes 3 mpc-4 mpc-5 mpc-6 ;;
    all)
      for s in expired-seq7 outage-queue outage-dispatching abort-1 abort-2 abort-3 concurrent-facility pool-sum pool-race pool-replay restart-all; do
        run "$s"
      done
      ;;
    *) echo "unknown scenario $1" >&2; exit 2 ;;
  esac
}

pin_makers() {
  # The room's automatic quote refresh rewrites a bot-held Maker policy at
  # the start of every round, and a rewritten policy needs a new standing
  # pool, which needs the seven-node FROST committee.  Pinning the Maker
  # seats to manual keeps the registered policies fixed, so a submission
  # while nodes are down exercises the corporate queue and nothing else.
  for m in 0 1 2 3; do
    acc force --seat "maker:$m" --manual "$1" --out "/out/force-maker$m-manual$1.json" > /dev/null || true
  done
  log "maker seats manual=$1"
}

# Whatever happens, the committee is left running: a scenario that dies with
# nodes stopped or paused would leave the demo network unusable.
restore_committee() {
  for svc in $NODES; do docker unpause "$(container "$svc")" > /dev/null 2>&1 || true; done
  compose start $NODES > /dev/null 2>&1 || true
}
trap restore_committee EXIT

log "project=$PROJECT network=$NETWORK image=$APP_IMAGE out=$OUT"
docker image inspect -f '{{.Id}}' "$APP_IMAGE" > "$OUT/app-image-id.txt"
docker ps --filter "name=$PROJECT" --format '{{.Names}}\t{{.Image}}\t{{.Status}}' > "$OUT/containers-at-start.txt"
pin_makers 1
for s in "$@"; do run "$s"; done
if [ "${QOMM_KEEP_PINNED:-0}" != 1 ]; then pin_makers 0; fi
log "done"
