#!/usr/bin/env bash
# Container entrypoint: starts every process of the integration test cluster,
# initializes it (idempotent) and waits. Marks the outcome with
# /dev/shm/cmom-ready or /dev/shm/cmom-failed (a tmpfs: a restarted container
# starts without them).
set -uo pipefail

DIR=$(cd "$(dirname "$0")" && pwd)
NODE=(bash "$DIR/node.sh")
rm -f /dev/shm/cmom-ready /dev/shm/cmom-failed
# Left over by a previous run of a restarted container: nothing runs yet.
rm -f /data/cmom/*.pid

shutdown() {
	"${NODE[@]}" stop-all
	exit 0
}
trap shutdown TERM INT

setup() {
	"${NODE[@]}" start-mongod &&
		CMOM_INIT_PHASE=replsets mongosh --nodb --quiet "$DIR/init.js" &&
		"${NODE[@]}" start-mongos &&
		CMOM_INIT_PHASE=cluster mongosh --nodb --quiet "$DIR/init.js"
}

if setup; then
	touch /dev/shm/cmom-ready
	echo "cmom-it-cluster ready"
else
	touch /dev/shm/cmom-failed
	echo "cmom-it-cluster setup failed" >&2
fi

sleep infinity &
wait $!
