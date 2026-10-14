#!/usr/bin/env bash
# Starts and stops the processes of the integration test cluster. Runs inside
# the cmom-it-cluster container (see cluster.sh).
#
# Every member is configured as localhost:<port> and the ports are published
# 1:1, so member addresses work both inside the container and from the host.
#
#   37017, 37018  mongos
#   37019         config server replica set "configRS" (1 member)
#   37021-37023   shard "shard01": primary, secondary, arbiter
#   37024-37025   shard "shard02": primary, passive secondary (priority 0)
#   37030         standalone mongod
#
# Usage: node.sh start|stop|wait|wait-secondary|wait-primary PORT
#        node.sh start-mongod|start-mongos|stop-all|status
set -euo pipefail

BASE=/data/cmom
MONGOD_PORTS=(37019 37021 37022 37023 37024 37025 37030)
MONGOS_PORTS=(37017 37018)

command_for() {
	case "$1" in
	37019) echo "mongod --configsvr --replSet configRS" ;;
	37021 | 37022 | 37023) echo "mongod --shardsvr --replSet shard01" ;;
	37024 | 37025) echo "mongod --shardsvr --replSet shard02" ;;
	37030) echo "mongod" ;;
	37017 | 37018) echo "mongos --configdb configRS/localhost:37019" ;;
	*)
		echo "unknown port: $1" >&2
		return 1
		;;
	esac
}

pid_of() {
	local file="$BASE/$1.pid"
	[[ -s $file ]] && cat "$file"
}

running() {
	local pid
	pid=$(pid_of "$1") || return 1
	kill -0 "$pid" 2>/dev/null
}

start() {
	local port=$1
	running "$port" && return 0
	local cmd
	read -r -a cmd <<<"$(command_for "$port")"
	mkdir -p "$BASE/$port" "$BASE/log"
	local extra=()
	if [[ ${cmd[0]} == mongod ]]; then
		extra+=(--dbpath "$BASE/$port" --wiredTigerCacheSizeGB 0.25)
		if [[ " ${cmd[*]} " == *" --replSet "* ]]; then
			extra+=(--oplogSize 128)
		fi
	fi
	"${cmd[@]}" "${extra[@]}" --port "$port" --bind_ip_all --fork \
		--logpath "$BASE/log/$port.log" --logappend \
		--pidfilepath "$BASE/$port.pid" \
		--setParameter diagnosticDataCollectionEnabled=false >/dev/null
}

stop() {
	local port=$1 pid
	pid=$(pid_of "$port") || return 0
	kill -TERM "$pid" 2>/dev/null || true
	for _ in $(seq 1 120); do
		kill -0 "$pid" 2>/dev/null || break
		sleep 0.5
	done
	if kill -0 "$pid" 2>/dev/null; then
		kill -KILL "$pid" 2>/dev/null || true
	fi
	rm -f "$BASE/$port.pid"
}

# Waits until the process on PORT answers a ping.
wait_ping() {
	local port=$1
	for _ in $(seq 1 240); do
		if mongosh --quiet --port "$port" --eval 'db.adminCommand({ ping: 1 }).ok' 2>/dev/null | grep -q '^1$'; then
			return 0
		fi
		sleep 0.5
	done
	echo "process on port $port did not answer" >&2
	return 1
}

# Waits until `db.hello().FIELD` is true on PORT.
wait_hello() {
	local port=$1 field=$2
	for _ in $(seq 1 240); do
		if mongosh --quiet --port "$port" --eval "db.hello().$field" 2>/dev/null | grep -q '^true$'; then
			return 0
		fi
		sleep 0.5
	done
	echo "member on port $port is not $field" >&2
	return 1
}

case "${1:-}" in
start)
	start "$2"
	wait_ping "$2"
	;;
stop) stop "$2" ;;
wait) wait_ping "$2" ;;
wait-secondary) wait_hello "$2" secondary ;;
wait-primary) wait_hello "$2" isWritablePrimary ;;
start-mongod)
	for port in "${MONGOD_PORTS[@]}"; do start "$port"; done
	;;
start-mongos)
	for port in "${MONGOS_PORTS[@]}"; do start "$port"; done
	for port in "${MONGOS_PORTS[@]}"; do wait_ping "$port"; done
	;;
stop-all)
	for port in "${MONGOS_PORTS[@]}" "${MONGOD_PORTS[@]}"; do stop "$port"; done
	;;
status)
	for port in "${MONGOS_PORTS[@]}" "${MONGOD_PORTS[@]}"; do
		if running "$port"; then
			echo "$port running (pid $(pid_of "$port"))"
		else
			echo "$port stopped"
		fi
	done
	;;
*)
	echo "usage: $0 start|stop|wait|wait-secondary|wait-primary PORT | start-mongod|start-mongos|stop-all|status" >&2
	exit 2
	;;
esac
