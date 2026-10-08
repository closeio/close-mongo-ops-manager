#!/usr/bin/env bash
# MongoDB test cluster for tests/mongo_integration.rs: two mongos, a config
# server replica set, two shards and a standalone mongod, all in one container
# (cmom-it-cluster) with every member configured as localhost:<port> and the
# ports published 1:1 on 127.0.0.1. See node.sh for the layout.
#
# Plus a standalone mongod in its own container (cmom-it-plain, port 37031)
# that keeps the container's random host name, like a development setup: the
# host it reports for its operations does not resolve from the host machine.
#
# Usage: tests/docker/cluster.sh up|down|status|env
#        tests/docker/cluster.sh stop-member|start-member PORT
#
#   up      create/start the cluster (idempotent), wait until it is ready and
#           print the environment variables for the integration tests
#   down    remove the containers and their data
#   status  show the state of every process
#   env     print the environment variables
set -euo pipefail

DIR=$(cd "$(dirname "$0")" && pwd)
NAME=cmom-it-cluster
PLAIN=cmom-it-plain
IMAGE=${CMOM_IT_IMAGE:-mongo:8.0}
PORTS=37017-37030
PLAIN_PORT=37031

# State of container $1 (default: the cluster).
state() {
	local status
	if status=$(docker inspect -f '{{.State.Status}}' "${1:-$NAME}" 2>/dev/null) && [[ -n $status ]]; then
		echo "$status"
	else
		echo missing
	fi
}

print_env() {
	cat <<EOF
export CMOM_IT_MONGOS_URI='mongodb://localhost:37017/'
export CMOM_IT_MONGOS_MULTI_URI='mongodb://localhost:37017,localhost:37018/'
export CMOM_IT_RS_URI='mongodb://localhost:37021,localhost:37022/?replicaSet=shard01'
export CMOM_IT_STANDALONE_URI='mongodb://localhost:37030/'
export CMOM_IT_PLAIN_URI='mongodb://localhost:$PLAIN_PORT/'
export CMOM_IT_CONTAINER='$NAME'
EOF
}

wait_ready() {
	echo "waiting for $NAME to be ready..." >&2
	for _ in $(seq 1 360); do
		if docker exec "$NAME" test -f /dev/shm/cmom-ready 2>/dev/null; then
			return 0
		fi
		if docker exec "$NAME" test -f /dev/shm/cmom-failed 2>/dev/null; then
			docker logs --tail 50 "$NAME" >&2
			echo "cluster setup failed (see: docker logs $NAME)" >&2
			return 1
		fi
		if [[ $(state) != running ]]; then
			docker logs --tail 50 "$NAME" >&2 || true
			echo "container $NAME is not running" >&2
			return 1
		fi
		sleep 1
	done
	echo "timed out waiting for $NAME" >&2
	return 1
}

# The standalone with a container host name; ready when it answers a ping.
up_plain() {
	case "$(state "$PLAIN")" in
	running) ;;
	missing)
		docker run -d --name "$PLAIN" -p "127.0.0.1:$PLAIN_PORT:27017" "$IMAGE" \
			--wiredTigerCacheSizeGB 0.25 >/dev/null
		;;
	*) docker start "$PLAIN" >/dev/null ;;
	esac
	for _ in $(seq 1 120); do
		if docker exec "$PLAIN" mongosh --quiet --eval 'db.adminCommand({ ping: 1 }).ok' 2>/dev/null | grep -q '^1$'; then
			return 0
		fi
		sleep 1
	done
	echo "container $PLAIN is not ready" >&2
	return 1
}

up() {
	up_plain
	case "$(state)" in
	running) ;;
	missing)
		docker run -d --name "$NAME" --hostname localhost \
			-p "127.0.0.1:$PORTS:$PORTS" \
			-v "$DIR:/cmom:ro" \
			--entrypoint bash "$IMAGE" /cmom/entrypoint.sh >/dev/null
		;;
	*) docker start "$NAME" >/dev/null ;;
	esac
	wait_ready
	print_env
}

case "${1:-}" in
up) up ;;
down) docker rm -f "$NAME" "$PLAIN" >/dev/null 2>&1 || true ;;
status)
	echo "container $PLAIN: $(state "$PLAIN")"
	echo "container: $(state)"
	if [[ $(state) == running ]]; then
		docker exec "$NAME" bash /cmom/node.sh status
		if docker exec "$NAME" test -f /dev/shm/cmom-ready; then echo "cluster: ready"; else echo "cluster: not ready"; fi
	fi
	;;
env) print_env ;;
stop-member) docker exec "$NAME" bash /cmom/node.sh stop "$2" ;;
start-member) docker exec "$NAME" bash /cmom/node.sh start "$2" ;;
*)
	echo "usage: $0 up|down|status|env|stop-member PORT|start-member PORT" >&2
	exit 2
	;;
esac
