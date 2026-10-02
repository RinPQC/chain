#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-rinpqc-devnet}"
export DEVNET_PREFIX="${DEVNET_PREFIX:-172.30.91}"
compose=(docker compose -f compose.yaml)
helper=/usr/local/lib/rinpqc-devnet.py
node() { [[ "${1:-}" =~ ^node[0-3]$ ]] || { echo 'Expected node0, node1, node2 or node3' >&2; exit 2; }; }
case "${1:-help}" in
  up) "${compose[@]}" up --build -d ;;
  down) "${compose[@]}" down ;;
  fresh)
    "${compose[@]}" build
    "${compose[@]}" run --rm --no-deps init initialize
    "${compose[@]}" run --rm --no-deps init assert-unused
    excluded="$("${compose[@]}" run --rm --no-deps init first-proposer)"
    services=()
    for i in 0 1 2 3; do [[ "$i" == "$excluded" ]] || services+=("node$i"); done
    "${compose[@]}" up -d "${services[@]}"
    echo "Initial proposer node$excluded is unused and offline; start it later with: scripts/devnet.sh start node$excluded"
    ;;
  first-proposer) "${compose[@]}" run --rm --no-deps init first-proposer ;;
  stop|restart) node "${2:-}"; "${compose[@]}" "$1" "$2" ;;
  start) node "${2:-}"; "${compose[@]}" up -d --no-deps "$2" ;;
  partition|reconnect)
    action="$1"; shift
    [[ "$#" -gt 0 ]] || { echo 'Specify nodes to isolate or reconnect' >&2; exit 2; }
    for service in "$@"; do
      node "$service"
      container="$("${compose[@]}" ps -q "$service")"
      [[ -n "$container" ]] || { echo "$service is not running" >&2; exit 1; }
      if [[ "$action" == partition ]]; then
        docker network disconnect "${COMPOSE_PROJECT_NAME}_consensus" "$container"
      else
        index="${service#node}"
        docker network connect --ip "${DEVNET_PREFIX}.$((10 + index))" "${COMPOSE_PROJECT_NAME}_consensus" "$container"
      fi
    done
    ;;
  status)
    for service in node0 node1 node2 node3; do
      echo "$service"
      "${compose[@]}" exec -T "$service" python3 "$helper" rpc metrics || echo "$service unavailable" >&2
    done
    ;;
  rpc) node "${2:-}"; service="$2"; shift 2; "${compose[@]}" exec -T "$service" python3 "$helper" rpc "$@" ;;
  pay|retry) "${compose[@]}" exec -T node0 python3 "$helper" "$1" "${2:?Specify amount or signed_file}" ;;
  logs) shift; "${compose[@]}" logs --no-color --tail 200 "$@" ;;
  reset)
    [[ "${2:-}" == --discard-test-state ]] || { echo 'Destructive reset requires: reset --discard-test-state' >&2; exit 2; }
    echo "Deleting disposable project $COMPOSE_PROJECT_NAME: containers, network and volumes ${COMPOSE_PROJECT_NAME}_node{0,1,2,3}. Keys, balances and history will be lost."
    "${compose[@]}" down --volumes
    ;;
  help|-h|--help) echo 'Usage: scripts/devnet.sh up|down|fresh|first-proposer|status|logs [node]|stop node|start node|restart node|partition nodes...|reconnect nodes...|rpc node method [ID]|pay amount|retry signed_file|reset --discard-test-state' ;;
  *) echo "Unknown devnet command: $1" >&2; exit 2 ;;
esac
